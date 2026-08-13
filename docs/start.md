<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Get started

AgentStack gives every agent CLI the same MCP servers, skills, and instructions
without copying their configuration into every project.

The simple model is:

```text
your library repo  →  each project's manifest + lock  →  every agent CLI
```

## 1. Install and set up this machine

```bash
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"       # only if the installer printed "Add to PATH"
```

The installer puts the binary in `/usr/local/bin` when it can write there and in
`$HOME/.local/bin` otherwise. It names the directory it chose, and when that
directory is not already on your `PATH` it prints
`Add to PATH:  export PATH="<dir>:$PATH"` — the `export` above is the
`~/.local/bin` case. Copy whichever line it prints into your shell startup file,
so a new shell still finds the binary.

That installs v0.19.0, the release these pages describe, and verifies the
download against the checksums published with it. It is the latest stable
release, so no version pin is needed; `AGENTSTACK_VERSION=vX.Y.Z` still asks
for a specific build. To build from a checkout instead, see
[Install](../README.md#install).

Set up this machine:

```bash
agentstack init --connect
```

A real run on a machine with Claude Code and Codex already configured — shown
here in its non-interactive form — prints this. Lines marked `…` are trimmed:

```console
$ agentstack init --connect --yes --secrets env
🔍  Found 2 coding tools · importing 2 MCP servers (github · tldraw)
🔐  1 plaintext token in your live CLI configs → ${GITHUB_PERSONAL_ACCESS_TOKEN} here; each value was COPIED, the original
    is still in its CLI's own config, unchanged
  ✓ pinned 2 servers
🔑  Stored 1 token in .env (gitignored)
✅  Wrote ~/my-project/.agentstack/agentstack.toml

Claude Code (~/.claude.json)
…   trimmed: the JSON diff that adds the one bridge entry
  ✓ gateway registered (agentstack more mcp --auto-project)

Codex CLI (~/.codex/config.toml)
…   trimmed: the TOML diff that adds the one bridge entry
  ✓ gateway registered (agentstack more mcp --auto-project)

Updated 2 harness configs.

Import complete.
  Manifest:  ~/my-project/.agentstack/agentstack.toml
  Imported:  2 MCP servers → library 'local', referenced by name
…   trimmed: a Note that no server was copied back into those CLI configs
  Next:      agentstack doctor   (check the result)
  Then:      undo: agentstack more restore --last --write
…   trimmed: the per-CLI --verbose pointers and the zero-files caveat
```

`more` is the extended toolbox — every `more` command also runs at its bare name.

Two details that vary with your machine rather than with the command:

- **The placeholder name is not fixed.** Each lifted token keeps the key name it
  had in the CLI config it came from, so you get `${GITHUB_PERSONAL_ACCESS_TOKEN}`
  above only because that is what the source config called it; a config using
  `GITHUB_TOKEN` yields `${GITHUB_TOKEN}`.
- **`(gitignored)` is a claim only inside a Git repository.** `init` writes the
  managed `/.agentstack/.env` line into `.gitignore` only when the project is one.
  Outside a Git repository the same run prints `Stored 1 token in .env` with no
  `(gitignored)` suffix and writes no `.gitignore`, leaving a plaintext token
  unprotected — run `git init` first, or add `/.agentstack/.env` to whatever
  ignore list you use, before that directory goes anywhere.

Then check the result:

```bash
agentstack status
```

On the project above, that prints:

```console
agentstack 0.19.0 — one portable manifest, every agent CLI

  CLIs      2 of 13 supported detected here: Claude Code · Codex CLI
  Setup  ~/my-project/.agentstack/agentstack.toml — 2 servers → 2 detected CLIs, no CLIs pinned
  Status    locked · untrusted
  Toolset   default — default; opens on the next trusted agent connection
  Delivery  skills + MCP servers served live to 2 CLIs
            0 project artifacts for the capabilities served live (the manifest and lock stay, and so does any managed region in a house-rules file)
            rendered lane: house rules + settings + hooks for 2 of 2 CLIs
  Context   2 declared servers not measured — context cost unknown, not zero   see `agentstack more report usage`

  Next:  agentstack doctor  ·  verify the wiring — every warning names its fix
  All commands: agentstack --help   ·   per-CLI detail: agentstack status --verbose
```
A scripted `init --yes` acknowledges the **import** and never the servers it
found, so the project is locked and *untrusted* — nothing it declares is active
yet. `status` says so and names the one step that changes it:

```bash
agentstack trust .          # at a terminal: read the review, answer it
```

Headless, that review is two commands — preview the surface, hand its digest
back:

```bash
agentstack trust --preview                      # JSON; read `surface_digest`
agentstack trust . --yes --consented <surface_digest>
```

At a terminal, plain `agentstack init` asks this question inside the wizard and
there is no separate step.


The last lines change with the state. When something still needs doing, the
`Next:` line names that instead — and `status` adds a
`Deep check (drift, quirks, supply chain): agentstack doctor` pointer, which the
ready output above does not print. Either way `agentstack doctor` is the deeper
check whenever you want it.

`init` detects your CLIs, offers to import the MCP server entries and supported
settings it can represent, and registers the zero-files gateway. It previews
changes before writing them. `status` tells you whether the setup is ready and
gives one next command when it is not.

It does not take over everything already installed:

| What already exists | What AgentStack does |
| --- | --- |
| MCP entries in a CLI's global or project config | Shows them in the import review and copies accepted definitions into the **machine library** (`~/.agentstack/lib`, or your first linked source) by default. The original entries are not deleted. |
| Skills already installed for Codex, Claude Code, or another CLI | Leaves them where they are. Adopt a skill explicitly when you want AgentStack to manage and share it. |
| App-owned tools such as Computer Use or a server installed inside another application's bundle | Leaves them with the owning app and names them as excluded. `--include-tool-managed` is an explicit override. |
| Unrelated CLI settings, plugins, and built-in capabilities | Leaves them untouched. AgentStack manages only the entries and managed regions it records. |

To preview adopting existing native skills into your machine library:

```bash
agentstack adopt --to-library
agentstack adopt --to-library --write
```

**Two things, two names, and the difference matters on day two.** The
**machine library** is `~/.agentstack/lib` — a store on this machine, created
for you, not version-controlled. A **library repo** is a Git checkout you own
and link, which is what travels to another machine. `init` writes into the
machine library; only what you publish into a library repo comes with you.

Use either for your own reusable skills, MCP definitions, and instructions.
Keep a capability inside one project only when it truly belongs to that
repository.

## 2. Link a library repo

Your library repo is a normal Git checkout. It can be private, live on any Git host,
and use a simple folder structure for reusable skills, MCP servers,
instructions, hooks, and extensions.

```bash
agentstack lib link ~/GitHub/ai-setup --name central --first
agentstack lib link ~/GitHub/ai-setup --name central --first --write
agentstack lib sources
```

The first command previews. The second links the folder on this machine.
`--first` makes it the default place to read and add reusable capabilities.

If you already have `~/.agentstack/lib`, AgentStack keeps it as another source
and you read the combined library. When two sources hold the same name, the
first one wins.

### Publish what `init` imported

Linking a repo does not move anything into it. `init` put the servers it found
in the **machine library**, which stays on this machine — so a second machine
that clones the repo receives nothing until you publish them:

```bash
agentstack lib add-server github \
  --file ~/.agentstack/lib/servers/github.toml --write
```

That copies the definition into the linked repo, checksums it, and prints where
it landed. Repeat per server (`agentstack lib list` names what the machine
library holds), then commit and push the repo. **This is the step that makes
day two work** — without it the next machine clones an empty library and every
name resolves to nothing.

See [Several libraries work together](library.md#several-libraries-work-together)
for the folder layout, collisions, and qualified names such as
`local:rust-testing`.

### Put reusable capabilities in it

Scaffold a new skill in the directory you are standing in, then copy it — or one
you already have — into the library:

```bash
agentstack lib new api-review
agentstack lib add ./api-review
agentstack lib add ./api-review --write
```

Add an MCP server definition the same way. It must contain `${REF}`
placeholders, never secret values:

```bash
agentstack lib add-server github --file ./github-server.toml
agentstack lib add-server github --file ./github-server.toml --write
agentstack lib list
```

`lib new` writes `./<name>/SKILL.md` into the **current working directory**, not
into the library — it prints `✓ scaffolded ./api-review/SKILL.md` and stops
there; `lib add` is the separate step that copies that folder in. Every command
previews first and writes only with `--write`. Commit and push the library as
you would any Git repo.

`lib list` prints what the combined library holds, and closes with a
**What is dead in here** section: every skill, server, extension, and hook with
no recorded usage, plus the reversible `agentstack lib remove <name> --write`
that retires one. It says `no data` rather than "unused" on purpose — counts
come from recorded history only, and the call log rotates.

## 3. Keep each project small

A normal project needs two committed files:

```text
your-project/
└── .agentstack/
    ├── agentstack.toml   # names what this project may use
    └── agentstack.lock   # pins the exact content it resolved
```

The manifest can stay this small:

```toml
version = 1
default_toolset = "rust"

[toolsets.rust]
servers = ["upstash/context7", "gha-search"]
skills = ["rust-best-practices", "rust-testing"]

[instructions.team-style]
targets = ["*"]
```

The names resolve from your linked library. The project does not copy those
files. A trusted new agent connection opens `rust` automatically.

The lock records the exact skill bodies, server definitions, and instruction
bodies selected from the library. Commit it with the manifest so another
machine resolves the same approved content.

## 4. Lock, review, done

After changing what a project selects:

```bash
agentstack lock          # preview
agentstack lock --write
agentstack trust .
agentstack status
```

`lock --write` accepts the exact pinned bytes and `trust .` is the human review
for this machine. Run the loop after you add or remove a selected capability,
accept an updated library item, or change the default toolset — not for
unrelated edits.

`trust .` needs a terminal: with stdin not a TTY it prints the review surface and
then refuses with `refusing to trust: stdin is not a terminal`. In CI, or in an
agent shell with no TTY, take the two-step form instead — `trust --preview`
already emits JSON (there is no `--json` flag), and the field to read out of it is
`surface_digest`, whose value already carries its `sha256:` prefix:

```bash
agentstack trust --preview                  # JSON review surface; read `surface_digest` from it
agentstack trust . --yes --consented <surface_digest>   # the value already includes sha256:
```

`--yes` requires `--consented`, and the grant refuses unless the digest still
matches the bytes on disk — so it is a consent bound to exactly what was
reviewed, not a way to skip the review.

```console
$ agentstack lock --write
✓ pinned 2 servers from 1 toolset in ~/my-project/.agentstack/agentstack.lock
  no configs rendered, no skills materialized — that stays `agentstack use --write`.

Next: `agentstack doctor` to verify the gateway wiring.
```

See [Trust a project](howto/trust-a-repo.md) for why the lock comes first.

`agentstack doctor` is the deeper verification. On a healthy project it ends
like this:

```console
$ agentstack doctor
Adapters & CLIs ✓ 2 detected, configs parse
Zero-files gateway ✓ 3 checks pass
Secrets ✓ 1 check pass
Reproducibility
  – reproducibility: nothing declared to check — no toolset here pulls a skill from the library
  ✓ github               library server · matches lock
  ✓ tldraw               library server · matches lock
· 12 sections for features this project doesn't use hidden, 3 sections with nothing to fix summarised — agentstack doctor --all shows every line.

0 errors, 0 warnings.
  ready: reviewed and verified — the default opens on the next agent connection
  next: nothing to repair — this setup is verified   the default toolset opens automatically on the next trusted agent connection
```

## 5. What the agent sees

Zero-files delivery does not dump every skill and MCP schema into the model's
context. The agent starts with skill names and one-line descriptions, calls
`agentstack_load(name, reason)` for one full body when a task matches, and
searches the gateway for an MCP tool schema only when it needs that tool.

Missing `.mcp.json` and local skill folders are therefore normal. House rules,
settings, hooks, extensions, and file-only CLIs still use managed files.

See [Dynamic skill loading](concepts.md#dynamic-skill-loading) for the whole
picture.

## 6. Set up another machine

```bash
agentstack up --library https://github.com/you/ai-setup.git
agentstack up --library https://github.com/you/ai-setup.git --write
agentstack status
```

Install the binary there with the same one-liner as
[step 1](#1-install-and-set-up-this-machine), then run those two commands: the
first previews the complete bootstrap, and the second clones or links the
library, detects this machine's CLIs, and installs the required global
integration. Two things do not travel with the repo — secret **values** and the
trust review are per machine, so this machine asks for its own missing secrets
and its own `agentstack trust .`.

Later, refresh with:

```bash
agentstack up            # preview
agentstack up --write
```

AgentStack works the same whether the CLI starts directly, from stock T3 Code,
or from another supervisor. T3 Code is not required.

## The commands you will actually use

| Command | Use it for |
| --- | --- |
| `agentstack status` | See what is ready and the one next action |
| `agentstack doctor` | Run the deeper verification when something is wrong |
| `agentstack lib sources` | See every linked library and name collision |
| `agentstack more delivery` | See where each capability goes, per CLI |
| `agentstack lock` | Preview content changes before accepting them |
| `agentstack trust .` | Review changed project content on this machine — needs a terminal; without one use `trust --preview` then `trust . --yes --consented <surface_digest>` |
| `agentstack up` | Preview a machine/library refresh |
| `agentstack undo` | Review and reverse AgentStack-managed writes |

## If Homebrew gives you an older build

These pages describe v0.19.0. The installer one-liner above serves it. The
Homebrew formula is published by hand after each stable release, so
`brew install` can trail the releases page for a while; `agentstack --version`
says which build you have.

On v0.17.1 the extended-toolbox prefix and the `yes`, `undo`, `why`, and
`unrender` verbs do not exist — they fail with `unrecognized subcommand` —
and every command that does exist there runs at its own bare name, for example
`agentstack gateway connect --all --write` rather than
`agentstack more gateway connect --all --write`. (In v0.18 the prefix was
spelled `agentstack x`; that spelling is still accepted, permanently.)

To upgrade, run `agentstack self update --write` — on v0.17.1 that is the
spelling, and plain `--help` there does not list `self` (only
`agentstack --help --all` does). The install line at the top of this page works
too, and is the one to use if `self update` reports that Homebrew owns the
binary.

## Why use it — and when not to

Use AgentStack when you have more than one agent CLI, project, machine, or
person and want one reviewed source of truth. It is especially useful when you
want reusable private skills, consistent MCP servers, per-machine secrets, and
no generated capability files in repositories.

You may not need it for one CLI with one small, permanent configuration and no
need to share or audit it. AgentStack adds a manifest, locking, and trust review
on purpose; those controls are valuable only when the setup is worth managing.

Next: [Central library](library.md) · [Concepts](concepts.md) ·
[Toolsets](howto/name-a-toolset.md) · [Workflows](workflows.md) ·
[Run a workflow](howto/run-a-workflow.md) · [FAQ](faq.md)
