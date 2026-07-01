# agentstack-test — a multi-project sandbox

A self-contained harness for exercising agentstack against a *simulated machine*
without touching your real `~/.claude.json`, `~/.codex/`, etc. Everything runs
under an isolated `HOME` inside `runtime/` (gitignored).

## Layout

```
as                      # run the binary against the simulated machine
demo-central-library.sh # end-to-end central-library walkthrough (see below)
fixtures/               # demo INPUT — not owned by any project, not the library
  central-library/
    kibana.toml         #   a server definition (${REF} secret only)
    sql-review/SKILL.md #   a skill
projects/
  web-app/              # frontend: figma + github servers, review skills, [profiles.review]
  api-service/          # backend:  postgres + github servers, api skills, [profiles.backend]
  data-pipeline/        # analytics: snowflake server, sql-review skill, [profiles.analytics]
  central-demo/         # ONLY agentstack.toml — references a server + skill BY NAME
runtime/                # (gitignored) simulated machine — created on first run
  home/.claude.json     #   a global `github` MCP server, shared by every project
  home/.codex/config.toml
  ashome/               #   agentstack state, sessions, history
  ashome/lib/           #   the ACTUAL central library (skills + server definitions)
```

Three distinct things, so the story is unambiguous: **`fixtures/`** = demo input ·
**`runtime/ashome/lib/`** = the real central library · **`projects/central-demo/`**
= a manifest only.

## Central library demo

`central-demo/` is **just a manifest** — a profile that references a server +
skill **by name**, with no inline definitions and no capability files of its own;
both resolve from the central library. The seed fixtures live in
`fixtures/central-library/` (outside any project), so it's clear the project
doesn't carry them. One script seeds the library from those fixtures and drives
the whole flow:

```sh
./demo-central-library.sh
```

It: (1) `lib add-server` + `lib add` from `fixtures/central-library/` into
`runtime/ashome/lib`, (2) `lib list`, (3) shows the by-name manifest,
(4) `use central --write` (resolving a `${REF}` secret from the env),
(5) `explain kibana` (origin/provenance/lock/secrets), (6) prints the resolved
server from the simulated `~/.claude.json`, and (7) shows the lock pinning the
server's **definition** digest only — never the secret value. A final optional
step runs `doctor` and shows the Reproducibility section. Idempotent; re-run any
time (first run against a fresh `runtime/` shows the full write; re-runs are
"up to date").

Two harnesses are simulated: **Claude Code** (has project scope) and **Codex**
(global-only) — so the global-vs-project distinction is visible.

## Use it

`./as` is just `agentstack` with `HOME`/`AGENTSTACK_HOME` pointed at `runtime/`:

```sh
./as --manifest-dir projects/web-app doctor          # detect harnesses, show drift
./as --manifest-dir projects/web-app dashboard       # open the web UI for this project
./as --manifest-dir projects/web-app session start review   # load a bundle for now…
./as --manifest-dir projects/web-app session end            # …then revert it
```

### See global vs project in the dashboard
Open the dashboard for `web-app`, go to **Servers**, and flip the **Global /
Project** switch up top:
- **Global** — `github` is on for every project; `figma` shows a `project` tag
  (set only in this repo).
- **Project** — `figma` is ✓; `github` shows a faded ✓ ("inherited from global").

### Drive it as an agent (MCP)
```sh
./as --manifest-dir projects/web-app mcp
```
then speak JSON-RPC: `agentstack_list_loadable` returns the skills the active
session allows; `agentstack_load` pulls one on demand (fenced + logged).

## Reset
Delete `runtime/` (or just `runtime/ashome` for state, `runtime/home/.claude.json`
for the simulated global config) and re-run any `./as` command to recreate it.
Per-project generated files (`.mcp.json`, `.claude/skills/`) are gitignored too.
