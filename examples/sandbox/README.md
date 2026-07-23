# examples/sandbox — a multi-project sandbox

A self-contained harness for exercising agentstack against a *simulated machine*
without touching your real `~/.claude.json`, `~/.codex/`, etc. Everything runs
under an isolated `HOME` inside `runtime/` (gitignored).

**Prerequisites:** only `demo-lockdown.sh` needs Docker running (it launches a
real `run --sandbox --lockdown` container). `demo-firstrun.sh`,
`demo-central-library.sh`, and plain `./as` commands need no Docker.

## Layout

```
as                      # run the binary against the simulated machine
demo-firstrun.sh        # clean first-run adoption story (see below)
demo-central-library.sh # end-to-end central-library walkthrough (see below)
demo-lockdown.sh        # no-direct-route sandbox: run --sandbox --lockdown (needs Docker)
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

## First-run demo

The clean adoption story on a *fresh machine*, fully fenced. It simulates a dev
who already runs Claude Code with one MCP server and adopts agentstack to spread
it across every other CLI:

```sh
./demo-firstrun.sh
```

Unlike `./as`, this script builds its **own** throwaway sandbox under
`runtime/firstrun/` and wipes it each run, so it is always a genuine first run
(never the pre-seeded `runtime/home`). It walks the core loop end to end —
`init → bootstrap → doctor --ci → apply → apply --write` — then proves the one
imported server landed, correctly translated, in all five CLI configs, and that
a re-run is a boring no-op with `doctor --ci` still green. The header comments
carry an `asciinema`/`vhs` recipe for turning it into a GIF.

To record it with [VHS](https://github.com/charmbracelet/vhs):

```sh
vhs demo-firstrun.tape
```

The GIF lands at `../../docs/firstrun.gif` — nothing embeds it, so review its
size before committing one. The replay actually shown on the site
(`docs/firstrun.svg`) is a condensed transcript maintained in
`tools/make-term-svgs.py`; regenerate it with `python3 tools/make-term-svgs.py`.
For an asciinema workflow, use the commands in the script header.

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
./as --manifest-dir projects/web-app session start review   # load a profile for now…
./as --manifest-dir projects/web-app session end            # …then revert it
```

### See global versus project state

Use `status`, `doctor`, and `diff --scope project` for the repository view.
Run the corresponding command with `--scope global` where supported to inspect
the machine layer.

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
