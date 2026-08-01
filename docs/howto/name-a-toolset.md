<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Name a toolset

A **toolset** is a named subset of the setup you already have — "backend",
"incident", "design" — that you activate together. In the manifest it is a
`[profiles.<name>]` block — the manifest key kept its original spelling so
existing manifests keep working, but everything you read and type says
*toolset*. It names *which* of your
servers and skills come along for a task; it is **not** a policy, a permission
level, or a workflow role. A manifest with no toolset named activates its whole
inline set, so you only name one once you want more than one.

Prerequisite: a project with an `.agentstack/agentstack.toml`
[manifest](../concepts.md) (run `agentstack init` if you don't have one).

## The one-command way

The capabilities are already in your manifest. A toolset just *names a subset*
of them — no re-import, no copying. The shortest path is one command:

```bash
agentstack toolset create --name backend --server postgres --server github --skill sql-review
```

At a terminal it shows what it will create, asks, and on yes writes the
`[profiles.backend]` block and re-locks. `--skill '*'` means every inline skill.

**Naming a toolset does not switch to it.** Nothing is rendered and none of your
CLIs change — you have defined a subset, not chosen it. Activate it when you
want it, with `agentstack session start backend` (see
[below](#which-activation-session-or-apply)). To undo the creation itself,
delete the `[profiles.backend]` block from the manifest.

Scripts and graphical clients get a two-step consent contract instead —
[reference: selective skills via toolsets](../reference.md#selective-skills-via-toolsets).

## Or write it by hand

A toolset is four lines of TOML, and reading it is often clearer than reading a
command. Add one `[profiles.<name>]` block that lists the servers and skills
that task needs:

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
resolved set — the toolset's servers plus exactly the skills the agent loaded —
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

A project toolset can be committed and deliberately minimal while your machine
manifest keeps a broader personal one — see [team setup](team-setup.md).
Neither grants extra authority: a toolset only selects from capabilities that
already passed review.

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

- [Concepts](../concepts.md) — toolset, manifest, delivery modes
- [Reference: selective skills via toolsets](../reference.md#selective-skills-via-toolsets)
- [Reference: ephemeral sessions](../reference.md#ephemeral-sessions-agentstack-session)
- [Team setup](team-setup.md) — project vs. machine manifests
