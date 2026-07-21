<img alt="agentstack" src="docs/logo.svg" width="380">

> **Cloning a repo shouldn't hand your agent to a stranger.**
> AgentStack puts everything your AI coding tools (Claude Code, Codex,
> Cursor, …) are allowed to run into one reviewed file. A repo you clone
> can't auto-activate any of it until you approve that repo — and what
> runs through AgentStack's gateway is firewalled and logged. Each run is
> labelled with [how strongly that is actually enforced](https://tarekkharsa.github.io/agentstack/enforcement.html).

**[Website](https://tarekkharsa.github.io/agentstack/)** ·
[Docs](https://tarekkharsa.github.io/agentstack/docs.html) ·
[Get started](https://tarekkharsa.github.io/agentstack/start.html) ·
[Releases](https://github.com/Tarekkharsa/agentstack/releases)

[![CI](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/ci.yml?branch=main&style=flat&label=CI)](https://github.com/Tarekkharsa/agentstack/actions/workflows/ci.yml) [![Conformance](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/conformance.yml?branch=main&style=flat&label=conformance)](https://github.com/Tarekkharsa/agentstack/actions/workflows/conformance.yml) [![Release](https://img.shields.io/github/v/release/Tarekkharsa/agentstack?style=flat&label=release)](https://github.com/Tarekkharsa/agentstack/releases) [![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-blue?style=flat)](https://github.com/Tarekkharsa/agentstack/blob/main/LICENSE-MIT)

## Why

Every skill, MCP (Model Context Protocol — the plugin standard agent CLIs use for tools) server, and
agent config you adopt is **unreviewed code plus instructions**, wired into a process that holds your
credentials, shell, and network. Adopting one is `npm install` with an agent attached — no lockfile,
no review gate, no record of what it did. AgentStack closes four gaps:

- **Anything a repo declares can run.** A clone stays *inert* until you trust its exact bytes; any edit re-gates it.
- **Nothing narrows or records what agents do.** Your machine policy — which no repo can loosen — fences tools, secrets, and egress, and every brokered call lands in an audit log.
- **Every CLI spells the same setup differently.** One reviewed [manifest](https://tarekkharsa.github.io/agentstack/concepts.html) renders them all; secrets stay references.
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
cargo build --release                  # add --features sandbox for `run --sandbox`
./target/release/agentstack self link  # symlink onto your PATH
```

Release binaries ship with sandbox support compiled in; a bare `cargo build` does not — pass
`--features sandbox` to get `run --sandbox` / `--lockdown`.

## Climb as far as you need

AgentStack is adopted in steps, not all at once — each pays off on its own, and nothing later is
required to keep the earlier wins:

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](https://tarekkharsa.github.io/agentstack/start.html) | `agentstack init` → `apply` | one reviewed manifest for every CLI; real tokens out of your config files |
| [2 — Verify](https://tarekkharsa.github.io/agentstack/start.html#s-verify) | `agentstack` · `agentstack doctor` | drift caught early; every warning names its exact fix |
| [3 — Guard](https://tarekkharsa.github.io/agentstack/start.html#s-guard) | `agentstack guard install` | `rm -rf`, `git reset --hard`, and `.env` reads blocked before they land |
| [4 — Trust](https://tarekkharsa.github.io/agentstack/howto/trust-a-repo.html) | `gateway connect` · `trust .` | cloned repos stay inert until you review them; brokered calls firewalled and audited |
| [5 — Scale](https://tarekkharsa.github.io/agentstack/reference.html#the-central-library) | profiles · `lib` · packs | one governed stack across projects, machines, and teammates |
| [6 — Confine](https://tarekkharsa.github.io/agentstack/howto/lock-down-a-run.html) | `run --sandbox --lockdown` | kernel-enforced confinement — the agent's only route out is the audited proxy |

## Documentation

Everything is explained on the website — that is the one place docs live:

- **[Get started](https://tarekkharsa.github.io/agentstack/start.html)** — guided setup, ~10 minutes, expected output at every step
- **[Concepts](https://tarekkharsa.github.io/agentstack/concepts.html)** — every term in two or three plain sentences
- **[Which mode do I need?](https://tarekkharsa.github.io/agentstack/choose.html)** — protection level and delivery mode, decided in two tables
- **[How-tos](https://tarekkharsa.github.io/agentstack/docs.html)** — add a server or skill, trust a repo, lock down a run, team setup, CI, undo
- **[See what your agents did](https://tarekkharsa.github.io/agentstack/howto/see-what-happened.html)** — runs, dashboard, optimize, explain
- **[Reference](https://tarekkharsa.github.io/agentstack/reference.html)** — the complete feature and command inventory

**Go deeper** — the [enforcement matrix](https://tarekkharsa.github.io/agentstack/enforcement.html) (what each mode actually enforces, checked against the source), the [architecture](https://tarekkharsa.github.io/agentstack/architecture.html) (how it works inside), the power how-tos ([lock down a run](https://tarekkharsa.github.io/agentstack/howto/lock-down-a-run.html), [team setup](https://tarekkharsa.github.io/agentstack/howto/team-setup.html), [CI](https://tarekkharsa.github.io/agentstack/howto/ci.html)), and [25 runnable walkthroughs](https://tarekkharsa.github.io/agentstack/examples.html).

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
