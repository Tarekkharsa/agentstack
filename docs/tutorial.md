<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Tutorial

The walkthrough now lives in one place: **[Get started](start.md)**. It runs
the whole path end to end — install, link a library, put reusable capabilities
in it, keep a project small, lock and trust, and bootstrap another machine —
with real captured command output at each step.

This page is the map: seven steps in the order you do them, each saying what
you are about to change and linking to the section that teaches it with real
command output.

## Step 1 — Set up this machine

Install the binary, let `init` find the CLIs you already have, and register the
bridge once so live delivery can reach them. This is the only step that touches
machine-wide state, and it is the one to get right before anything else.
→ [Install and set up this machine](start.md#1-install-and-set-up-this-machine)

## Step 2 — Link your reusable library

Point AgentStack at a folder you own and want to reuse across projects. Linking
is not copying: the library stays yours, and any folder can be one, so nothing
is moved into a location the tool controls.
→ [Link a library repo](start.md#2-link-a-library-repo)

## Step 3 — Put reusable skills and servers in it

Author a skill or a server definition once, in the library, with secrets held as
`${REF}` placeholders rather than values. Everything downstream selects it by
name, so this is the only place its details are written down.
→ [Put reusable capabilities in it](start.md#put-reusable-capabilities-in-it)

## Step 4 — Select names in a project

A project's manifest names what it needs and nothing more — no copied commands,
no duplicated tokens. This is what keeps a repository small and readable while
still carrying the whole setup.
→ [Keep each project small](start.md#3-keep-each-project-small)

## Step 5 — Pin and review it

`lock --write` pins exact commits and content digests; `trust .` then shows you
what those pinned bytes will run and asks for your consent. The order matters —
locking after trusting invalidates the grant you just made.
→ [Lock, review, done](start.md#4-lock-review-done)

## Step 6 — Start your agent normally

Launch your coding CLI the way you always do. On an MCP-capable tool the
capabilities arrive live, with no file written into the project, which is why
there is often nothing on disk to inspect.
→ [What the agent sees](start.md#5-what-the-agent-sees)

## Step 7 — Bootstrap another machine

On a second machine, one command syncs the library, connects the installed CLIs,
and verifies the checkout against its lockfile — the payoff for having pinned
and reviewed the setup in the first place.
→ [Set up another machine](start.md#6-set-up-another-machine)

## Deeper on one topic

- [Add an MCP server](howto/add-a-server.md) — one reusable definition, secrets
  by reference.
- [Add a skill](howto/add-a-skill.md) — author it once, select it by name.
- [Name a toolset](howto/name-a-toolset.md) — the command form of a toolset,
  and how a default is chosen.
- [Trust a project](howto/trust-a-repo.md) — why the lock comes first, and what
  the review actually shows.
- [Undo AgentStack changes](howto/undo.md) — the timeline, and taking a managed
  write back.
- [Team setup](howto/team-setup.md) — the same library across people and CI.

Next: [Get started](start.md) · [Central library](library.md) ·
[FAQ](faq.md) · [Full reference](reference.md)
