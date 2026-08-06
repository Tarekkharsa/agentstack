<img alt="agentstack" src="docs/logo.svg" width="380">

> **One agent setup. Every coding CLI.**
> AgentStack collects the MCP servers, skills, and instructions you already use
> into one `.agentstack/` directory, then **serves them live** to Claude Code,
> Codex, Cursor, Gemini CLI, OpenCode, and
> [eight more](https://tarekkharsa.github.io/agentstack/adapters.html) — so the
> project stays clean. What no live channel can carry, and every tool without
> MCP, is written into native files instead. Named toolsets let you switch by
> project or task; doctor, diff, and restore keep every change understandable
> and recoverable.

**[Website](https://tarekkharsa.github.io/agentstack/)** ·
[Docs](https://tarekkharsa.github.io/agentstack/docs.html) ·
[Get started](https://tarekkharsa.github.io/agentstack/start.html) ·
[Releases](https://github.com/Tarekkharsa/agentstack/releases)

[![CI](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/ci.yml?branch=main&style=flat&label=CI)](https://github.com/Tarekkharsa/agentstack/actions/workflows/ci.yml) [![Conformance](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/conformance.yml?branch=main&style=flat&label=conformance)](https://github.com/Tarekkharsa/agentstack/actions/workflows/conformance.yml) [![Release](https://img.shields.io/github/v/release/Tarekkharsa/agentstack?style=flat&label=release)](https://github.com/Tarekkharsa/agentstack/releases) [![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-blue?style=flat)](https://github.com/Tarekkharsa/agentstack/blob/main/LICENSE-MIT)

## Try it in 60 seconds

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
agentstack init                          # finds what your CLIs already have, writes it into .agentstack/
agentstack x gateway connect --all --write # register the bridge once, so live delivery reaches your CLIs
agentstack status                        # is it ready — and if not, the one thing that fixes it
```

Every command above is in the release the install line serves. A few newer
verbs shorten some of this; they are gathered in one place —
[newer than the stable release](https://tarekkharsa.github.io/agentstack/start.html#newer-than-the-stable-release).
`agentstack --version` says which release you have.

That is the whole first run. Here is what it left in your project — plain files
you can open, read, and commit:

```text
your-project/
├── .agentstack/
│   ├── agentstack.toml   # everything your tools may run: servers, skills, instructions
│   ├── agentstack.lock   # the pins — exact commits and content digests (written by `agentstack lock --write`)
│   └── .env              # token values lifted out of your configs (only when init found any)
└── .gitignore            # one managed line, so that .env is never committed
```

That is normally the whole footprint. On MCP-capable tools a project carries
only `.agentstack/` — no `.mcp.json`, no `.claude/skills/`, no generated
`CLAUDE.md`.

`.agentstack/agentstack.toml` records your whole setup. It is called
the **[manifest](https://tarekkharsa.github.io/agentstack/concepts.html)**, and
everything AgentStack delivers is delivered from it. Mostly the system writes it
for you: `init` and `add` fill it in, and so does
[dropping a skill folder in](https://tarekkharsa.github.io/agentstack/howto/add-a-skill.html).
It holds `${GITHUB_TOKEN}`-style placeholders, never the token values.

### How it reaches your tools

Delivery is **routed, not chosen** — AgentStack picks the lane per capability
and per tool, and `agentstack x delivery` prints the routing:

- **Served live.** Skills and MCP servers go to MCP-capable tools through one
  gateway, brokered, policy-checked, digest-verified, and recorded. Nothing is
  written into the project for them.
- **rendered lane:** instructions, settings, hooks, and extensions are written
  into native files — always, because no live channel a tool is known to
  consume can carry them correctly — and so is *every* capability on a tool
  without MCP. Those files are regenerable and can be taken back off at any
  time.

Live delivery needs the bridge registered once per tool:

```sh
agentstack x gateway connect --all           # preview which CLIs would be registered
agentstack x gateway connect --all --write   # register it
```

Until that registration exists, nothing is served live: `status` and `delivery`
both say `planned live (not connected)`, and `doctor` reports the gap as an
error — `no bridge for <the CLIs missing it> — nothing routed live is reaching
them` — naming the command above.
Files in the rendered lane are written either way.

That same directory arriving in a repository you *cloned* behaves differently: it
stays inert until you review it — no server spawns, no skill enters an agent's
context, no secret resolves, and no file is written for it either. `apply --write`
and `use --write` refuse to render an untrusted project's servers, skills,
instructions, hooks, and extensions, and name `agentstack trust .` as the fix;
editing the manifest or the lock afterwards drops the project back to
untrusted until you review it again. Running `init` yourself needs no such
step — building the setup *is* the consent. And nothing a project declares can
loosen the limits your own machine sets.

`init` is a guided wizard. Scripting or CI?
[Use it in CI](https://tarekkharsa.github.io/agentstack/howto/ci.html).

Here is the whole loop, condensed from a real run of the current binary:

![Two CLIs with different half-setups: agentstack imports both into one manifest, connects the gateway so both CLIs are served the servers live while the project stays clean, passes doctor with 0 errors, renders each native format on request, and restores the machine byte-for-byte](docs/demos/first-value.svg)

1. **Start** — two real native configs: Claude Code knows a `github` server
   (inline token), Codex knows `tldraw`. Neither knows the other's.
2. **Import** — `agentstack init --yes --secrets env`: one manifest; the token
   is copied to a gitignored `.env` and referenced as `${GITHUB_TOKEN}` (your
   CLI's own config keeps its copy until you apply at global scope).
3. **Connect** — `agentstack x gateway connect --all --write`: the live lane
   needs one bridge registered per MCP-capable CLI.
4. **Route** — `agentstack x delivery`: both CLIs are MCP-capable, so the servers
   are served live and no file is written for them. The project holds
   `.agentstack/` and the `.gitignore` that hides the lifted secret — no native
   config at all.
5. **Verify** — `agentstack doctor`: 0 errors, 0 warnings. On your own machine
   expect a note or two — advisories like "these servers launch via bare `npx`"
   are stated once and do not count against readiness; a first Codex project
   also warns until you open Codex there once and accept its trust prompt.
6. **Render anyway** — `agentstack x delivery render-locally --write`, then
   `agentstack apply --toolset default --scope global --write`: the rendered
   lane is routed, not removed, so asking for files is an explicit opt-in. Both
   CLIs then carry both servers, each in its own format.
7. **Undo** — `agentstack x restore --last --write`, four times (the render, the
   render-locally override, the bridge, the import): every file byte-identical
   to where it started.

Reproduce it yourself, fenced (an isolated temp `HOME` — it never touches your
real configs, and it asserts every step, so it doubles as the witness that this
output stays accurate):
[`examples/first-value-demo/run-demo.sh`](examples/first-value-demo/run-demo.sh).

## Why

Every agent CLI has its own configuration format, file locations, and way to
install the same underlying capabilities. AgentStack gives the whole lifecycle
one source of truth:

- **Stop repeating configuration.** Import what you already have; one manifest
  then reaches every supported CLI — served live where it can be, rendered into
  that CLI's native format where it cannot.
- **Switch by project or task.** A toolset is a named subset of your setup;
  temporary sessions activate it and restore the previous native files afterward.
- **Understand and repair drift.** `doctor` finds the problem, `diff` shows the
  consequence, `adopt` keeps an intentional hand edit, and `restore` undoes a
  bad change.
- **Share without sharing credentials.** Manifests and lockfiles contain
  `${REF}` placeholders, never secret values; each machine supplies its own.
- **Stay safe as setups become portable.** Unfamiliar repository declarations
  stay inert until reviewed, machine policy remains the ceiling, and governed
  calls are recorded.

Using one CLI with a small hand-managed setup? You may not need AgentStack yet.
It becomes useful when you repeat the same setup across tools, projects,
machines, or teammates.

## Install

The one-line installer above verifies the release tarball against the `checksums.txt` published with
each release. Or build from a checkout:

```sh
cargo build --release                  # add --features sandbox for `run --sandbox`
./target/release/agentstack x self link  # symlink onto your PATH
```

Release binaries ship with sandbox support compiled in; a bare `cargo build` does not — pass
`--features sandbox` to get `run --sandbox` / `--lockdown`.

Once installed, `agentstack x self update` moves you to the latest release; it verifies the
download against the release's published checksum before replacing anything.

There is also a Homebrew tap, `Tarekkharsa/homebrew-tap`, whose
`Formula/agentstack.rb` currently pins v0.17.1 — the latest published release:

```sh
brew install Tarekkharsa/tap/agentstack
```

The formula is published by hand after each release, so it can lag a tag; if
`brew info` shows an older version than the
[releases page](https://github.com/Tarekkharsa/agentstack/releases), use the
installer or a checkout. On a Homebrew install, upgrade with
`brew upgrade agentstack` rather than `agentstack x self update` — replacing the
file directly desynchronizes the formula.

**Supported platforms: macOS and Linux.** A Windows binary is published, but it is not
exercised by CI and the codebase carries almost no Windows-specific handling — treat it as
untested rather than supported. If you use it and it works, say so in an issue; that is the
evidence that would move it.

### Upgrading

`agentstack x self update` previews; `--write` verifies the sha256 before
installing.
[Details](https://tarekkharsa.github.io/agentstack/reference.html).

## Grow into it

Start with configuration portability. Add toolsets, sharing, and stronger
governance only when you need them:

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](https://tarekkharsa.github.io/agentstack/start.html) | `agentstack init` → `gateway connect --all --write` | import once, delivered everywhere |
| [2 — Switch](https://tarekkharsa.github.io/agentstack/howto/name-a-toolset.html) | toolsets · `session start/end` | toolsets and temporary sessions |
| [3 — Diagnose](https://tarekkharsa.github.io/agentstack/start.html#verify-it) | `agentstack doctor` · `diff` | doctor and diff explain drift |
| [4 — Recover](https://tarekkharsa.github.io/agentstack/howto/undo.html) | `adopt` · `apply` · `restore` · `uninstall` | keep an edit, or undo the write |
| [5 — Share](https://tarekkharsa.github.io/agentstack/howto/team-setup.html) | manifest · lock · library | locked, secret-free setups |
| [6 — Govern](https://tarekkharsa.github.io/agentstack/howto/trust-a-repo.html) | trust · policy · lockdown | trust, policy, confined runs |

## The command surface

`agentstack --help` lists the fifteen everyday verbs — `init`, `status`, `add`,
`search`, `apply`, `doctor`, `lock`, `toolset`, `use`, `yes`, `run`, `trust`,
`undo`, `adopt`, `secret`. Four ideas cover them: Setup, Toolset, Status, Undo.

A verb is on that screen when the product itself can tell you to run it — a
first-run step, a `doctor` fix line, or a machine-readable `next_action`.

Two of them — `yes` and `undo` — plus `agentstack x why` and
`agentstack x unrender` below are in v0.18.0 and later, which is newer than the
v0.17.1 the tap and the installer serve; on that binary they are an
`unrecognized subcommand` error. `agentstack --version` says which you have, and
[newer than the stable release](https://tarekkharsa.github.io/agentstack/start.html#newer-than-the-stable-release)
lists the full set.

`agentstack x why <name>` is the one to reach for when nothing is on disk: under
the default routing a served capability writes no file, so `why` is where its
origin, pin, approval, live tools and reach are stated. `agentstack x unrender`
is the opposite direction — it takes back a server config the rendered lane
left behind.

Everything else lives one hop away under `agentstack x`:

```bash
agentstack x                 # the rest of the toolbox, grouped by task
agentstack x guard install   # same command as `agentstack guard install`
```

Nothing was removed. Every command still runs at its own name with its own
`--help`, and `agentstack --help --all` prints the whole tree.

## Documentation

Everything is explained on the website — that is the one place docs live:

- **[Get started](https://tarekkharsa.github.io/agentstack/start.html)** — guided setup, ~10 minutes, expected output at every step
- **[Concepts](https://tarekkharsa.github.io/agentstack/concepts.html)** — every term in two or three plain sentences
- **[Which protection do I need?](https://tarekkharsa.github.io/agentstack/choose.html)** — how much protection to ask for; delivery is routed for you, not chosen
- **[How-tos](https://tarekkharsa.github.io/agentstack/docs.html)** — add a server or skill, trust a repo, lock down a run, team setup, CI, undo
- **[Migration recipes](https://tarekkharsa.github.io/agentstack/migrations.html)** — Claude + Codex, Cursor + Gemini, dotfiles, teams without shared secrets, complete removal
- **[Troubleshooting](https://tarekkharsa.github.io/agentstack/troubleshooting.html)** — search for the error text you got; every message is quoted from the binary and paired with its fix
- **[FAQ](https://tarekkharsa.github.io/agentstack/faq.html)** — will it overwrite my configs, is my API key in the manifest, do I need Docker
- **[Integrations](https://tarekkharsa.github.io/agentstack/integrations.html)** — use AgentStack from t3code and other graphical launchers; the current contract and limits
- **[See what your agents did](https://tarekkharsa.github.io/agentstack/howto/see-what-happened.html)** — runs, reports, optimize, explain
- **[Reference](https://tarekkharsa.github.io/agentstack/reference.html)** — the complete feature and command inventory
- **[Adapter support matrix](https://tarekkharsa.github.io/agentstack/adapters.html)** — which of the thirteen CLIs is verified nightly against the real tool, which is best-effort, and what each one manages

**Go deeper** — the [enforcement matrix](https://tarekkharsa.github.io/agentstack/enforcement.html) (what each mode actually enforces, checked against the source), the [architecture](https://tarekkharsa.github.io/agentstack/architecture.html) (how it works inside), and [18 runnable walkthroughs](https://tarekkharsa.github.io/agentstack/examples.html).

## Develop

```bash
cargo test              # unit + golden + integration
cargo clippy --all-targets
cargo fmt --check
```

Install your build with `agentstack x self link`. Ground rules and the security invariants:
[CONTRIBUTING.md](CONTRIBUTING.md). Release history: [CHANGELOG.md](CHANGELOG.md).

## Community and support

Bug reports and focused contributions are welcome. Read the
[Code of Conduct](CODE_OF_CONDUCT.md), [governance and succession
policy](GOVERNANCE.md), and [support scope](SUPPORT.md). Report vulnerabilities
privately through [SECURITY.md](SECURITY.md), never in a public issue.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
