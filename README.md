<img alt="agentstack" src="docs/logo.svg" width="380">

> **One agent setup. Every coding CLI.**
> Today you configure the same MCP server once per tool, in a different format
> each time, with your tokens sitting in plain JSON.
> With AgentStack the whole setup — config, skills, servers, instructions —
> lives in one repo you own, any machine gets it with one command, and every CLI
> speaks it: Claude Code, Codex, Cursor, Gemini CLI, OpenCode and
> [eight more](https://tarekkharsa.github.io/agentstack/adapters.html) — served
> live where the tool speaks MCP, written as native files where it does not.

**[Website](https://tarekkharsa.github.io/agentstack/)** ·
[Docs](https://tarekkharsa.github.io/agentstack/docs.html) ·
[Get started](https://tarekkharsa.github.io/agentstack/start.html) ·
[Releases](https://github.com/Tarekkharsa/agentstack/releases)

[![CI](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/ci.yml?branch=main&style=flat&label=CI)](https://github.com/Tarekkharsa/agentstack/actions/workflows/ci.yml) [![Conformance](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/conformance.yml?style=flat&label=conformance)](https://github.com/Tarekkharsa/agentstack/actions/workflows/conformance.yml) [![Release](https://img.shields.io/github/v/release/Tarekkharsa/agentstack?style=flat&label=release)](https://github.com/Tarekkharsa/agentstack/releases) [![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-blue?style=flat)](https://github.com/Tarekkharsa/agentstack/blob/main/LICENSE-MIT)

## Try it in 60 seconds

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"          # only if the installer printed "Add to PATH"
agentstack init                               # finds what your CLIs already have, writes it into .agentstack/
agentstack more gateway connect --all --write # register the bridge once (the interactive wizard may have done it)
agentstack status                             # is it ready — and if not, the one thing that fixes it
# then restart your coding CLI — a harness reads its config at startup:
agentstack more why <server>                  # names which CLIs are served it live
```

`more` is the extended toolbox. Every `more` command also runs at its bare name.

If you ran `agentstack init` interactively and let the wizard register the
gateway, step 3 has nothing left to do and prints `already connected` or
`Updated 0 harness configs`. That is success; the bridge only needs registering
once per tool.

The installer puts the binary in `/usr/local/bin` when it can write there and in
`$HOME/.local/bin` otherwise. It names the directory it chose, and when that
directory is not already on your `PATH` it prints
`Add to PATH:  export PATH="<dir>:$PATH"`. Copy that printed line (the `export`
above is the `~/.local/bin` case) into your shell startup file, so a new shell
still finds the binary.

![Two CLIs start with different half-setups. agentstack imports both into one manifest and connects the gateway, so both CLIs receive the servers live while the project stays clean; it then passes doctor with 0 errors, renders each native format on request, and restores the machine byte-for-byte](docs/demos/first-value.svg)

That is the whole first run. Here is what it left in your project — plain files
you can open, read, and commit:

```text
your-project/
├── .agentstack/
│   ├── agentstack.toml   # everything your tools may run: servers, skills, instructions
│   ├── agentstack.lock   # the pins — exact commits and content digests (`init` writes it; `agentstack lock --write` re-pins later changes)
│   └── .env              # token values lifted out of your configs (only when init found any)
└── .gitignore            # one managed line, so that .env is never committed
```

That `.gitignore` line is written **only inside a Git repository**: outside one
`init` still writes the plaintext `.env` and reports `Stored 1 token in .env`
without the `(gitignored)` suffix, so run `git init` before `init` — or add
`/.agentstack/.env` to whatever ignore list you do use — before that directory
goes anywhere.

That is normally the whole footprint. On MCP-capable tools a project carries
only `.agentstack/`, with no `.mcp.json`, no `.claude/skills/`, and no generated
`CLAUDE.md`.

`.agentstack/agentstack.toml` records your whole setup. It is called
the **[manifest](https://tarekkharsa.github.io/agentstack/concepts.html)**, and
everything AgentStack delivers is delivered from it. Mostly the system writes it
for you: `init` and `add` fill it in, and so does
[dropping a skill folder in](https://tarekkharsa.github.io/agentstack/howto/add-a-skill.html).
It holds `${GITHUB_TOKEN}`-style placeholders, never the token values.

### How it reaches your tools

Delivery is **routed, not chosen** — you never pick a delivery mode per server.
AgentStack decides the lane for each capability on each tool, and
`agentstack more delivery` prints what it decided. There are two lanes:

- **Served live.** Skills and MCP servers go to MCP-capable tools through one
  gateway — brokered, policy-checked, digest-verified, and recorded. Nothing is
  written into the project for them.
- **Written as files.** Instructions, settings, hooks, and extensions are
  written into each tool's native config — always, because no live channel a
  tool is known to consume can carry them correctly — and so is *every*
  capability on a tool without MCP. Those files are regenerable and can be
  taken back off at any time.

Live delivery needs the bridge registered once per tool:

```sh
agentstack more gateway connect --all           # preview which CLIs would be registered
agentstack more gateway connect --all --write   # register it
```

Until that registration exists, nothing is served live: `status` and `delivery`
both say `planned live (not connected)`, and `doctor` reports the gap as an
error — `no bridge for <the CLIs missing it> — nothing routed live is reaching
them` — naming the command above.
Files in the rendered lane are written either way.

That same directory arriving in a repository you *cloned* behaves differently,
because it stays inert until you review it. No server spawns, no skill enters an
agent's context, no secret resolves, and no file is written for it either.
`apply --write` and `use --write` refuse to render an untrusted project's
servers, skills, instructions, hooks, and extensions, and name
`agentstack trust .` as the fix; editing the manifest or the lock afterwards
drops the project out of trust — `status` then reads
`trust stale (content changed)` — until you review it again.

Running `init` yourself needs no such step. The guided wizard shows you the
plan, and your confirm grants the review for the bytes it just wrote, so
`status` reads `trusted` straight after it; building the setup *is* the consent.

A scripted import is the same deal in two commands, because `--yes`
acknowledges the write and not the servers. `agentstack init --plan` prints the
plan and its `plan_digest`, and `agentstack init --yes --consented <digest>`
imports and grants against the plan you read. Without the digest the import
still happens and the project simply stays untrusted, one `agentstack trust .`
away. Either way the consent is bound to those exact bytes and nothing wider,
and every later edit still comes back through `agentstack trust .`. And nothing
a project declares can loosen the limits your own machine sets.

`init` is a guided wizard. Scripting or CI?
[Use it in CI](https://tarekkharsa.github.io/agentstack/howto/ci.html).

The diagram at the top is that whole loop, condensed from a real run of the
current binary. Step by step:

1. **Start** — two real native configs: Claude Code knows a `github` server
   (inline token), Codex knows `tldraw`. Neither knows the other's.
2. **Import** — `agentstack init --yes --secrets env`: one manifest; the token
   is copied to a gitignored `.env` and referenced by a `${REF}` placeholder
   (your CLI's own config keeps its copy until you apply at global scope). The
   placeholder reuses the key name the token had in the CLI config it came
   from, so it is `${GITHUB_TOKEN}` in this fixture and
   `${GITHUB_PERSONAL_ACCESS_TOKEN}` if that is what your config calls it.
3. **Review** — `agentstack trust .`: nothing here runs until the setup is
   reviewed. At a terminal the wizard asks this inside `init`; a scripted run
   consents in two commands, previewing what would be granted and handing back
   its digest.
4. **Connect** — `agentstack more gateway connect --all --write`: the live lane
   needs one bridge registered per MCP-capable CLI.
5. **Route** — `agentstack more delivery`: both CLIs are MCP-capable, so the servers
   are served live and no file is written for them. The project holds
   `.agentstack/` and the `.gitignore` that hides the lifted secret. No native
   config at all.
6. **Verify** — `agentstack doctor`: 0 errors, 0 warnings. On your own machine
   expect a note or two. Advisories like "these servers launch via bare `npx`"
   are stated once and do not count against readiness; a first Codex project
   also warns until you open Codex there once and accept its trust prompt.
7. **Render anyway** — `agentstack more delivery render-locally --write`, then
   `agentstack apply --toolset default --scope global --write`: the rendered
   lane is still there on request, so asking for files is an explicit opt-in.
   Both CLIs then carry both servers, each in its own format.
8. **Undo** — `agentstack more restore --last --write`, four times (the render, the
   render-locally override, the bridge, the import): every file byte-identical
   to where it started.

Reproduce it yourself, fenced (an isolated temp `HOME`; it never touches your
real configs, and it asserts every step, so it doubles as the witness that this
output stays accurate):
[`examples/first-value-demo/run-demo.sh`](examples/first-value-demo/run-demo.sh).

## Your second machine

Once that setup is **published into a library repo and committed** — `init`
imports into the machine library at `~/.agentstack/lib`, which does not travel,
so `agentstack more lib add-server <name> --file ~/.agentstack/lib/servers/<name>.toml --write`
is what puts a definition in the repo — the next machine is one command:

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
agentstack up --library <git-url>           # preview what this machine would get
agentstack up --library <git-url> --write   # apply it
```

`up` syncs the library, finds the CLIs installed here, registers the bridge, and
verifies `agentstack.lock` against what you pinned. Two things it does not carry
for you: secret **values** and the trust review are per machine, so it names the
secrets still missing and leaves `agentstack trust .` to you.

## Why

Every agent CLI has its own configuration format, file locations, and way to
install the same underlying capabilities. AgentStack gives the whole lifecycle
one source of truth:

- **Stop repeating configuration.** Import what you already have; one manifest
  then reaches every supported CLI, served live where it can be and rendered
  into that CLI's native format where it cannot.
- **Switch by project or task.** A toolset is a named subset of your setup;
  temporary sessions activate it and restore the previous native files afterward.
- `doctor` finds the problem, `diff` shows the consequence, `adopt` keeps an
  intentional hand edit, and `restore` undoes a bad change.
- **Share without sharing credentials.** Manifests and lockfiles carry `${REF}`
  placeholders only; each machine supplies its own secret values.
- Repository declarations stay inert until you review them, machine policy
  remains the ceiling, and governed calls are recorded.

Using one CLI with a small hand-managed setup? You may not need AgentStack yet.
It becomes useful when you repeat the same setup across tools, projects,
machines, or teammates.

## Install

The quickstart installs **v0.19.0**, the release this README describes, and
every command on this page runs on it. It is the latest stable release, so the
installer needs no version pin; `AGENTSTACK_VERSION=vX.Y.Z` still asks for a
specific build, and `agentstack --version` says which one you have.

The one-line installer verifies the release tarball against the `checksums.txt` published with
each release. Each release also carries a GitHub build provenance attestation tying the asset to this
repository and the workflow that built it. Check it with
`gh attestation verify agentstack-<target>.tar.gz --repo Tarekkharsa/agentstack`. That establishes
*where* the artifact was built and does not replace the checksum comparison; see
[`RELEASING.md`](RELEASING.md). Or build from a checkout:

```sh
cargo build --release                  # add --features sandbox for `run --sandbox`
./target/release/agentstack more self link  # symlink onto your PATH
```

Release binaries ship with sandbox support compiled in; a bare `cargo build` does not. Pass
`--features sandbox` to get `run --sandbox` / `--lockdown`.

Once installed, `agentstack more self update` moves you to the latest *stable*
release; it verifies the download against the release's published checksum
before replacing anything. It never moves you onto a pre-release, and never
back off one. To install a specific build, pass `AGENTSTACK_VERSION`.

There is also a Homebrew tap, `Tarekkharsa/homebrew-tap`, carrying
`Formula/agentstack.rb`:

```sh
brew install Tarekkharsa/tap/agentstack
```

The formula is published by hand after each stable release, so it can lag a tag; if
`brew info` shows an older version than the
[releases page](https://github.com/Tarekkharsa/agentstack/releases), use the
installer or a checkout. On a Homebrew install, upgrade with
`brew upgrade agentstack` rather than `agentstack more self update`, because
replacing the file directly desynchronizes the formula.

**Supported platforms: macOS and Linux.** A Windows binary is published, but it is not
exercised by CI and the codebase carries almost no Windows-specific handling. Treat it as
untested rather than supported. If you use it and it works, say so in an issue; that is the
evidence that would move it.

### Upgrading

`agentstack more self update` previews; `--write` verifies the sha256 before
installing.
[Details](https://tarekkharsa.github.io/agentstack/reference.html).

## Grow into it

Start with configuration portability. Add toolsets, sharing, and stronger
governance only when you need them:

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](https://tarekkharsa.github.io/agentstack/start.html) | `agentstack init` → `gateway connect --all --write` | import once, delivered everywhere |
| [2 — Switch](https://tarekkharsa.github.io/agentstack/howto/name-a-toolset.html) | toolsets · `session start/end` | toolsets and temporary sessions |
| [3 — Diagnose](https://tarekkharsa.github.io/agentstack/start.html#the-commands-you-will-actually-use) | `agentstack doctor` · `diff` | doctor and diff explain drift |
| [4 — Recover](https://tarekkharsa.github.io/agentstack/howto/undo.html) | `adopt` · `apply` · `restore` · `uninstall` | keep an edit, or undo the write |
| [5 — Share](https://tarekkharsa.github.io/agentstack/howto/team-setup.html) | manifest · lock · library | locked, secret-free setups |
| [6 — Govern](https://tarekkharsa.github.io/agentstack/howto/trust-a-repo.html) | trust · policy · lockdown | trust, policy, confined runs |

## The commands

`agentstack --help` lists the sixteen everyday verbs — `init`, `up`, `status`,
`add`, `search`, `apply`, `doctor`, `lock`, `toolset`, `use`, `yes`, `run`,
`trust`, `undo`, `adopt`, `secret`. Four ideas cover them: Setup, Toolset,
Status, Undo.

A verb is on that screen when the product itself can tell you to run it — a
first-run step, a `doctor` fix line, or a machine-readable `next_action`.

`agentstack more why <name>` is the one to reach for when nothing is on disk: under
the default routing a served capability writes no file, so `why` is where its
origin, pin, approval, live tools and reach are stated. `agentstack more unrender`
is the opposite direction; it takes back a server config the rendered lane
left behind.

Everything else lives one hop away under `agentstack more`:

```bash
agentstack more                       # the rest of the toolbox, grouped by task
agentstack more guard install --write # same command as `agentstack guard install --write`
```

Nothing was removed. Every command still runs at its own name with its own
`--help`, and `agentstack --help --all` prints the whole tree.

The extended toolbox, and the `yes`, `undo`, `why`, and `unrender` verbs,
arrived in v0.18.0, where the prefix was spelled `agentstack x`. That spelling
still works and always will; `more` is what the help screens teach now. On
v0.17.1 none of it exists. The prefix is an `unrecognized subcommand` error, and
every command that does exist there runs at its own bare name. So if
`brew info` still shows the older formula, that is what you have.
`agentstack --version` settles it.

## Documentation

Everything is explained on the website, the one place docs live:

- **[Get started](https://tarekkharsa.github.io/agentstack/start.html)** — guided setup, ~10 minutes, expected output at every step
- **[Concepts](https://tarekkharsa.github.io/agentstack/concepts.html)** — every term in two or three plain sentences
- **[Which protection do I need?](https://tarekkharsa.github.io/agentstack/choose.html)** — how much protection to ask for; AgentStack routes delivery for you
- **[How-tos](https://tarekkharsa.github.io/agentstack/docs.html)** — add a server or skill, trust a repo, lock down a run, team setup, CI, undo
- **[Migration recipes](https://tarekkharsa.github.io/agentstack/migrations.html)** — Claude + Codex, Cursor + Gemini, dotfiles, teams without shared secrets, complete removal
- **[Troubleshooting](https://tarekkharsa.github.io/agentstack/troubleshooting.html)** — search for the error text you got; every message is quoted from the binary and paired with its fix
- **[FAQ](https://tarekkharsa.github.io/agentstack/faq.html)** — will it overwrite my configs, is my API key in the manifest, do I need Docker
- **[Integrations](https://tarekkharsa.github.io/agentstack/integrations.html)** — use AgentStack from t3code and other graphical launchers; the current contract and limits
- **[See what your agents did](https://tarekkharsa.github.io/agentstack/howto/see-what-happened.html)** — runs, reports, optimize, explain
- **[Reference](https://tarekkharsa.github.io/agentstack/reference.html)** — the complete feature and command inventory
- **[Adapter support matrix](https://tarekkharsa.github.io/agentstack/adapters.html)** — which of the thirteen CLIs is verified nightly against the real tool, which is best-effort, and what each one manages

For more depth, see the [enforcement matrix](https://tarekkharsa.github.io/agentstack/enforcement.html) (what each mode actually enforces, checked against the source), the [architecture](https://tarekkharsa.github.io/agentstack/architecture.html) (how it works inside), and [16 runnable walkthroughs](https://tarekkharsa.github.io/agentstack/examples.html).

## Develop

[CONTRIBUTING.md](CONTRIBUTING.md) has the fast inner loop, every CI gate with
the command that reproduces it locally, and the security invariants a change
has to preserve. Install your build with `agentstack more self link`. Release
history: [CHANGELOG.md](CHANGELOG.md).

## Community and support

Bug reports and focused contributions are welcome. Read the
[Code of Conduct](CODE_OF_CONDUCT.md), [governance and succession
policy](GOVERNANCE.md), and [support scope](SUPPORT.md). Report vulnerabilities
privately through [SECURITY.md](SECURITY.md), never in a public issue.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
