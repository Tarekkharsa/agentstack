<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Get started

One agent setup, shared by every coding CLI you use. This page is the whole
first hour: install it, run it once, connect the bridge, check the result, name
a toolset, switch between toolsets, and undo anything you did.

Everything here is a command from the current stable release. Where a newer
release has a shorter path, it is marked and gathered in one place at the
bottom — see [newer than the stable release](#newer-than-the-stable-release).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
```

The installer verifies the release tarball against the `checksums.txt`
published with that release. To build from a checkout instead:

```sh
cargo build --release                  # add --features sandbox for `run --sandbox`
./target/release/agentstack x self link  # symlink it onto your PATH
```

Confirm what you got, and which binary a bare `agentstack` runs:

```sh
agentstack --version
agentstack x self which
```

macOS and Linux are the supported platforms. A Windows binary is published but
is not exercised by CI — treat it as untested.

## What the first run does

```sh
cd your-project
agentstack init
```

`init` is a guided wizard. It detects the agent CLIs you have, imports the MCP
servers and skills they are already configured with, lifts any inline token
into a `${REF}` placeholder, asks where those values should live, then previews
what it would write and asks before writing anything.

What it leaves in your project is small, and it is all you commit:

```text
your-project/
├── .agentstack/
│   ├── agentstack.toml   # the manifest: servers, skills, instructions, toolsets
│   ├── agentstack.lock   # the pins — exact commits and digests (written by `agentstack lock --write`)
│   └── .env              # token values lifted out of your configs (only if init found any)
└── .gitignore            # one managed line, so .env is never committed
```

The manifest holds `${GITHUB_TOKEN}`-style placeholders, never token values.

Scripting it instead of answering prompts? `agentstack init --yes` writes the
manifest without prompting, `--secrets env|keychain|skip` decides where lifted
values go, and `agentstack init --plan` writes nothing at all and emits the
detection as JSON. See [use it in CI](howto/ci.md).

## Register the bridge

Skills and MCP servers are **served live** to the CLIs that can take them,
through one gateway registered once per CLI. Until that registration exists,
nothing is served live — the plan says where each capability is routed, but no
tool is receiving anything.

```sh
agentstack x gateway connect --all           # preview: which CLIs would be registered
agentstack x gateway connect --all --write   # register it
```

After this, every trusted project you `cd` into brings its own servers and
skills to those CLIs with no per-project files.

What is **not** served live is written to files, automatically and always:
instructions, settings, hooks, and extensions, plus every capability on a CLI
that has no MCP support. That is the rendered lane, and it is normal for a
project to be in both lanes at once.

```sh
agentstack x delivery        # the routing, per tool and per capability kind
```

The routing report names both lanes separately — the live one, and a
`rendered lane:` line for what went into files. If the bridge is not
registered, the live entries read `planned live (not connected)`, and the
report tells you the one command that connects it.

To write files even where the live channel would have worked:

```sh
agentstack x delivery render-locally
```

## Verify it

```sh
agentstack status    # where this project stands, and the one next step
agentstack doctor    # the deep check: what is wired, what is missing, what changed
```

`status` is one screen. `doctor` is the thorough pass — it names every problem
with the command that fixes it, and `doctor --ci` exits nonzero so a pipeline
can gate on it. When something is routed live and a CLI has no bridge
registered, `doctor` reports it as an error — `no bridge for <those CLIs> —
nothing routed live is reaching them` — and names
`agentstack x gateway connect --all --write`. A project that routes nothing live
gets no such error.

Expect a note or two on a real machine. Advisories — "these servers launch via
bare `npx`", for instance — are stated once and do not count against
readiness.

If the project came from someone else, it stays inert until you review it: no
server spawns, no skill enters an agent's context, no secret resolves.

```sh
agentstack trust .   # read the review, then approve it
```

See [trust a repo](howto/trust-a-repo.md) for what the review shows.

## Name a toolset

A toolset is a named subset of this project's servers and skills — one for
backend work, one for incident response — so you switch context without
editing five config files.

```sh
agentstack toolset create backend --server github --skill code-review
agentstack toolset list          # what is declared, and whether each is ready
```

In the manifest that is one table:

```toml
[toolsets.backend]
servers = ["github"]
skills  = ["code-review"]
```

## Switch between toolsets

```sh
agentstack use backend           # preview what activating it changes
agentstack use backend --write   # activate it
agentstack use --list            # every toolset, with a readiness flag
```

With one toolset declared, `agentstack use` picks it for you; with none
declared, every inline skill and server activates.

When you change what a toolset references, re-pin and re-review:

```sh
agentstack lock --write   # pin the new refs into agentstack.lock
agentstack trust .        # the lock is part of the consent surface, so re-approve
```

`agentstack lock` on its own previews: it prints the pins it would add, change,
or remove and writes nothing into the project. Computing the preview still
resolves sources, so git-backed sources are fetched. `--write` pins them.

## Undo anything

Every write is recorded before it lands, so every write can be taken back.

```sh
agentstack x restore                  # everything undoable, newest first
agentstack x restore --last --write   # undo the most recent write
agentstack x restore a1b2 --write     # undo one write by its id prefix
```

To reverse everything agentstack rendered, everywhere:

```sh
agentstack x uninstall          # preview: what would come off
agentstack x uninstall --write  # do it
```

Your `agentstack.toml` stays where it is, so `agentstack apply --write` brings
the whole setup back. Full detail: [undo anything](howto/undo.md).

## Newer than the stable release

The install line above serves the current stable release. These verbs are in
v0.18.0 and later, and `agentstack --version` says which you have:

| In v0.18.0 and later | What it shortens |
| --- | --- |
| `agentstack yes` | drop a skill or instruction file in `.agentstack/`, then declare + pin + trust + render behind one review |
| `agentstack undo` | the recorded changes as a timeline — pick a point and revert to it (`restore` does it one write at a time) |
| `agentstack x up` | set a new machine up from a setup that already exists (`apply --write` then `doctor` today) |
| `agentstack x share` / `agentstack x receive` | move a setup between people as a signed bundle (committing the manifest does it today) |

`agentstack x self update --write` moves you to the newest published release
after verifying its checksum.

## Where to go next

- [Concepts](concepts.md) — manifest, lockfile, toolset, trust, delivery lanes
- [Add a server](howto/add-a-server.md) · [Add a skill](howto/add-a-skill.md)
- [Name a toolset](howto/name-a-toolset.md) — the longer version of the section above
- [Trust a repo](howto/trust-a-repo.md) — what the review shows, and why
- [Share one setup with your team](howto/team-setup.md)
- [Use it in CI](howto/ci.md)
- [Troubleshooting](troubleshooting.md) — search for the error text you got
- [Reference](reference.md) — the complete command inventory
