<img alt="agentstack" src="docs/logo.svg" width="380">

> **One agent setup. Every coding CLI.**
> AgentStack imports the MCP servers, skills, and instructions you already use,
> keeps them in one portable manifest, and renders the right native configuration
> for Claude Code, Codex, Cursor, Gemini CLI, OpenCode, and more. Named toolsets
> let you switch by project or task; doctor, diff, and restore keep every
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

## Try it in 60 seconds

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
agentstack init      # your CLI configs → one reviewed manifest, previewed and applied
agentstack doctor    # verify it landed — every warning names its exact fix
```

`init` is a guided wizard. Scripting or CI? `agentstack init --secrets skip` writes only the manifest
— no prompts, no token values — then `agentstack apply --write`. Inline tokens are lifted into
`${REF}` placeholders, resolved per machine and never stored in the manifest.

Here is the whole loop, recorded from a real run of the current binary:

![Two CLIs with different half-setups: agentstack imports both into one manifest, renders each native format, passes doctor with 0 errors, and restores the machine byte-for-byte](docs/demos/first-value.gif)

1. **Start** — two real native configs: Claude Code knows a `github` server
   (inline token), Codex knows `tldraw`. Neither knows the other's.
2. **Import** — `agentstack init --yes --secrets env`: one manifest; the token
   is copied to a gitignored `.env` and referenced as `${GITHUB_TOKEN}` (your
   CLI's own config keeps its copy until you apply at global scope).
3. **Render** — `agentstack apply --scope global --write`: both CLIs now carry
   both servers, each in its own format.
4. **Verify** — `agentstack doctor`: 0 errors. On your own machine expect a
   note or two — advisories like "these servers launch via bare `npx`" are
   stated once and do not count against readiness; a first Codex project also
   warns until you open Codex there once and accept its trust prompt.
5. **Undo** — `agentstack restore --last --write`, twice: every file
   byte-identical to where it started.

Reproduce it yourself, fenced (an isolated temp `HOME` — it never touches your
real configs, and it asserts every step, so it doubles as the witness that this
output stays accurate):
[`examples/first-value-demo/run-demo.sh`](examples/first-value-demo/run-demo.sh).

## Install

The one-line installer above verifies the release tarball against the `checksums.txt` published with
each release. Or build from a checkout:

```sh
cargo build --release                  # add --features sandbox for `run --sandbox`
./target/release/agentstack self link  # symlink onto your PATH
```

Release binaries ship with sandbox support compiled in; a bare `cargo build` does not — pass
`--features sandbox` to get `run --sandbox` / `--lockdown`.

### Upgrading

```sh
agentstack self update           # what a newer release would install; downloads nothing
agentstack self update --write   # download, verify the sha256, install it
```

Like every other mutating command it previews by default. The archive is verified against the
release's published `checksums.txt` **before** it is unpacked or moved into place; a mismatch aborts
and leaves your existing binary untouched. That proves the transfer, not the provenance of the
release — for provenance, `gh attestation verify <asset> --repo Tarekkharsa/agentstack`.

A Homebrew install upgrades with `brew upgrade agentstack`, a source build with `cargo build
--release`, and a binary in a directory you cannot write needs `sudo` — each is detected and
explained before anything is downloaded.

`agentstack doctor` shows a one-line note when a newer release exists (a note, never a warning:
it cannot make a healthy setup look unhealthy). The check is cached for 24 hours, never blocks, and
is silent offline. Turn it off entirely with `AGENTSTACK_NO_UPDATE_CHECK=1`.

## Grow into it

Start with configuration portability. Add toolsets, sharing, and stronger
governance only when you need them:

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](https://tarekkharsa.github.io/agentstack/start.html) | `agentstack init` → `apply` | one manifest rendered correctly for every CLI |
| 2 — Switch | toolsets · `session start/end` | task-specific toolsets without permanent config pollution |
| [3 — Diagnose](https://tarekkharsa.github.io/agentstack/start.html#s-verify) | `agentstack doctor` · `diff` | drift explained before anything changes |
| [4 — Recover](https://tarekkharsa.github.io/agentstack/howto/undo.html) | `adopt` · `apply` · `restore` · `uninstall` | keep intentional edits, reconcile output, undo one change — or take all of it back off |
| [5 — Share](https://tarekkharsa.github.io/agentstack/howto/team-setup.html) | manifest · lock · library | reproducible setups across projects, machines, and teammates |
| [6 — Govern](https://tarekkharsa.github.io/agentstack/howto/trust-a-repo.html) | trust · policy · lockdown | reviewed activation and stronger enforced execution when needed |

## Documentation

Everything is explained on the website — that is the one place docs live:

- **[Get started](https://tarekkharsa.github.io/agentstack/start.html)** — guided setup, ~10 minutes, expected output at every step
- **[Concepts](https://tarekkharsa.github.io/agentstack/concepts.html)** — every term in two or three plain sentences
- **[Which mode do I need?](https://tarekkharsa.github.io/agentstack/choose.html)** — protection level and delivery mode, decided in two tables
- **[How-tos](https://tarekkharsa.github.io/agentstack/docs.html)** — add a server or skill, trust a repo, lock down a run, team setup, CI, undo
- **[Troubleshooting](https://tarekkharsa.github.io/agentstack/troubleshooting.html)** — search for the error text you got; every message is quoted from the binary and paired with its fix
- **[FAQ](https://tarekkharsa.github.io/agentstack/faq.html)** — will it overwrite my configs, is my API key in the manifest, do I need Docker
- **[Integrations](https://tarekkharsa.github.io/agentstack/integrations.html)** — use AgentStack from t3code and other graphical launchers; the current contract and limits
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
