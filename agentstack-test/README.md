# agentstack-test — a multi-project sandbox

A self-contained harness for exercising agentstack against a *simulated machine*
without touching your real `~/.claude.json`, `~/.codex/`, etc. Everything runs
under an isolated `HOME` inside `runtime/` (gitignored).

## Layout

```
as                      # run the binary against the simulated machine
projects/
  web-app/              # frontend: figma + github servers, review skills, [profiles.review]
  api-service/          # backend:  postgres + github servers, api skills, [profiles.backend]
  data-pipeline/        # analytics: snowflake server, sql-review skill, [profiles.analytics]
runtime/                # (gitignored) simulated machine — created on first run
  home/.claude.json     #   a global `github` MCP server, shared by every project
  home/.codex/config.toml
  ashome/               #   agentstack state, sessions, history
```

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
