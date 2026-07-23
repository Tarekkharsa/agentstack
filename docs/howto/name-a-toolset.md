<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Name a toolset

A **toolset** is a named subset of the setup you already have — "backend",
"incident", "design" — that you activate together. In the manifest it is a
`[profiles.<name>]` block; the CLI and the JSON contracts call it a *profile*,
the t3code panel calls it a *Toolset*. Same object. It names *which* of your
servers and skills come along for a task; it is **not** a policy, a permission
level, or a workflow role. A manifest with no toolset named activates its whole
inline set, so you only name one once you want more than one.

Prerequisite: a project with an `.agentstack/agentstack.toml`
[manifest](../concepts.md) (run `agentstack init` if you don't have one).

## Make a second toolset from what you already have

The capabilities are already in your manifest. A second toolset just *names a
subset* of them — no re-import, no copying. Add one `[profiles.<name>]` block
that lists the servers and skills that task needs:

```toml
# .agentstack/agentstack.toml — you already have these servers and skills.
[servers.postgres]      # ...
[servers.github]        # ...
[skills.sql-review]     # ...
[skills.oncall-runbook] # ...

# A new toolset: name the subset "backend" needs. Nothing else changes.
[profiles.backend]
servers = ["postgres", "github"]
skills  = ["sql-review"]
```

Then activate it — temporarily for a task, or applied on disk:

```bash
agentstack use --list                 # see every toolset and its readiness
agentstack session start backend      # use it for now; `session end` reverts
agentstack use backend --write        # or apply it on disk (stable/offline)
```

Prefer to **capture what you actually used** instead of writing the list by
hand? During a session, `agentstack session freeze --name backend` pins the
resolved set — the profile's servers plus exactly the skills the agent loaded —
into a new toolset you can replay deterministically.

## Two toolsets, two tasks

**Backend development vs. incident response.** Everyday coding wants your
database and code servers and the review skills; a 2 a.m. page wants read-only
observability and the runbook, and nothing that can write:

```toml
[profiles.backend]
servers = ["postgres", "github"]
skills  = ["sql-review", "api-conventions"]

[profiles.incident]
servers = ["grafana", "logs"]
skills  = ["oncall-runbook"]
```

`agentstack session start incident` for the duration of the page, then
`agentstack session end` puts every file back exactly as it was — the incident
tools never linger in your everyday setup.

**A minimal project toolset vs. a broad personal one.** Check a lean toolset
into a repo so a teammate gets precisely what the project needs; keep your
wider, personal set in your [machine manifest](team-setup.md) for your own work:

```toml
# ./.agentstack/agentstack.toml — committed, deliberately minimal
[profiles.project]
servers = ["github"]
skills  = ["repo-conventions"]
```

```toml
# ~/.agentstack/agentstack.toml — yours, broader
[profiles.personal]
servers = ["github", "postgres", "search"]
skills  = ["sql-review", "pdf", "notes"]
```

The project toolset travels with the repo and stays small; the personal one is
yours across every project. Neither grants extra authority — a toolset only
selects from capabilities that already passed review.

## Which activation: session or apply

- **Beginner path — use it temporarily.** `agentstack session start <name>`
  renders the toolset, and `agentstack session end` restores every native file
  to its pre-session bytes. Nothing lingers between tasks, and an interrupted
  session is always one `session end` from clean — this is the recommended way
  to switch toolsets.
- **Stable / offline path — apply it.** `agentstack use <name> --write` renders
  the toolset onto disk and leaves it there. Reach for this when you want the
  configuration to persist without a live agentstack around — a CI runner, an
  offline machine, a long-lived checkout.

Both are reversible: a session reverts on `end`, and an applied toolset is
undone with [`agentstack restore`](undo.md).

- [Concepts](../concepts.md) — profile (toolset), manifest, delivery modes
- [Reference: selective skills via profiles](../reference.md#selective-skills-via-profiles)
- [Reference: ephemeral sessions](../reference.md#ephemeral-sessions-agentstack-session)
- [Team setup](team-setup.md) — project vs. machine manifests
