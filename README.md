<img alt="agentstack" src="docs/logo.svg" width="380">

> **One agent setup. Every coding CLI.**
> AgentStack imports the MCP servers, skills, and instructions you already use,
> keeps them in one portable manifest, and renders the right native configuration
> for Claude Code, Codex, Cursor, Gemini CLI, OpenCode, and more. Profiles let
> you switch toolsets by project or task; doctor, diff, and restore keep every
> change understandable and recoverable.

Portable does not mean automatic: configuration from an unfamiliar repository
stays inert until you review it, and no project can loosen your machine policy.

**[Website](https://tarekkharsa.github.io/agentstack/)** ·
[Docs](https://tarekkharsa.github.io/agentstack/docs.html) ·
[Get started](https://tarekkharsa.github.io/agentstack/start.html) ·
[Releases](https://github.com/Tarekkharsa/agentstack/releases)

[![CI](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/ci.yml?branch=main&style=flat&label=CI)](https://github.com/Tarekkharsa/agentstack/actions/workflows/ci.yml) [![Conformance](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/conformance.yml?branch=main&style=flat&label=conformance)](https://github.com/Tarekkharsa/agentstack/actions/workflows/conformance.yml) [![Release](https://img.shields.io/github/v/release/Tarekkharsa/agentstack?style=flat&label=release)](https://github.com/Tarekkharsa/agentstack/releases) [![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-blue?style=flat)](https://github.com/Tarekkharsa/agentstack/blob/main/LICENSE-MIT)

## Why

Every agent CLI has its own configuration format, file locations, and way to
install the same underlying capabilities. AgentStack gives the whole lifecycle
one source of truth:

- **Stop repeating configuration.** Import what you already have, then render
  one [manifest](https://tarekkharsa.github.io/agentstack/concepts.html) into
  every supported CLI's native format.
- **Switch by project or task.** Profiles select a named toolset; temporary
  sessions activate it and restore the previous native files afterward.
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

## t3code is the graphical path

The CLI is complete on its own, but t3code is AgentStack's primary graphical
integration and launch channel. The native panel is being built around four
plain-language jobs: **Setup, Toolset, Status, and Undo**. It uses the same
CLI-owned plans and fixed actions as the terminal; t3code presents the
experience but never becomes the enforcement boundary.

Today, AgentStack already manages the native CLI configurations t3code launches,
doctor checks the supervisor integration, and shims can attach per-session run
identity. The guided setup/toolset panel and consent/admin contract are active
roadmap work and remain fail-closed until complete.

Read the honest current contract and limits:
[Use with t3code](https://tarekkharsa.github.io/agentstack/howto/use-with-t3code.html).

## Install

The one-line installer above verifies the release tarball against the `checksums.txt` published with
each release. Or build from a checkout:

```sh
cargo build --release                  # add --features sandbox for `run --sandbox`
./target/release/agentstack self link  # symlink onto your PATH
```

Release binaries ship with sandbox support compiled in; a bare `cargo build` does not — pass
`--features sandbox` to get `run --sandbox` / `--lockdown`.

## Grow into it

Start with configuration portability. Add profiles, sharing, and stronger
governance only when you need them:

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](https://tarekkharsa.github.io/agentstack/start.html) | `agentstack init` → `apply` | one manifest rendered correctly for every CLI |
| 2 — Switch | profiles · `session start/end` | task-specific toolsets without permanent config pollution |
| [3 — Diagnose](https://tarekkharsa.github.io/agentstack/start.html#s-verify) | `agentstack doctor` · `diff` | drift explained before anything changes |
| [4 — Recover](https://tarekkharsa.github.io/agentstack/howto/undo.html) | `adopt` · `apply` · `restore` | keep intentional edits, reconcile output, or undo safely |
| [5 — Share](https://tarekkharsa.github.io/agentstack/howto/team-setup.html) | manifest · lock · library | reproducible setups across projects, machines, and teammates |
| [6 — Govern](https://tarekkharsa.github.io/agentstack/howto/trust-a-repo.html) | trust · policy · lockdown | reviewed activation and stronger enforced execution when needed |

## Documentation

Everything is explained on the website — that is the one place docs live:

- **[Get started](https://tarekkharsa.github.io/agentstack/start.html)** — guided setup, ~10 minutes, expected output at every step
- **[Concepts](https://tarekkharsa.github.io/agentstack/concepts.html)** — every term in two or three plain sentences
- **[Which mode do I need?](https://tarekkharsa.github.io/agentstack/choose.html)** — protection level and delivery mode, decided in two tables
- **[How-tos](https://tarekkharsa.github.io/agentstack/docs.html)** — add a server or skill, trust a repo, lock down a run, team setup, CI, undo
- **[Use with t3code](https://tarekkharsa.github.io/agentstack/howto/use-with-t3code.html)** — current integration, native panel direction, and limits
- **[See what your agents did](https://tarekkharsa.github.io/agentstack/howto/see-what-happened.html)** — runs, reports, optimize, explain
- **[Reference](https://tarekkharsa.github.io/agentstack/reference.html)** — the complete feature and command inventory

**Go deeper** — the [enforcement matrix](https://tarekkharsa.github.io/agentstack/enforcement.html) (what each mode actually enforces, checked against the source), the [architecture](https://tarekkharsa.github.io/agentstack/architecture.html) (how it works inside), the power how-tos ([lock down a run](https://tarekkharsa.github.io/agentstack/howto/lock-down-a-run.html), [team setup](https://tarekkharsa.github.io/agentstack/howto/team-setup.html), [CI](https://tarekkharsa.github.io/agentstack/howto/ci.html)), and [18 runnable walkthroughs](https://tarekkharsa.github.io/agentstack/examples.html).

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
