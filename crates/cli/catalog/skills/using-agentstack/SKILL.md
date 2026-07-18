---
name: using-agentstack
description: Operate agentstack — manage a project's MCP servers, skills, profiles, and secrets from its manifest; work through the trust-gated runtime gateway; interpret policy denials; activate/deactivate capability sets; load skills from the central library; propose new capabilities safely.
---

# Using agentstack

Use this skill when a task involves an agent CLI's setup: adding/removing MCP
servers or skills, switching capability profiles, missing servers/skills in a
project, secrets that don't resolve, a proxied tool call being refused, or
auditing what an agent can access.

## The mental model (one paragraph)

agentstack is a **compiler and a runtime**. Compiler: intent lives in a
commit-safe manifest (`.agentstack/agentstack.toml`, or legacy root
`agentstack.toml`) and is rendered into each CLI's native config. Runtime: the
same manifest can be served live through a **gateway** (`agentstack mcp`) that
proxies the project's MCP servers to any harness — trust-gated per repo,
firewalled by two policy layers, every call logged. Skills and server
definitions can live in a machine-wide **central library** (`~/.agentstack/lib/`)
and be referenced **by name** (pinned by digest in `agentstack.lock`; the
gateway refuses a library definition that drifted from its pin). Secrets are
`${REF}` placeholders resolved per machine (env → varlock → OS keychain →
`.env`); an unresolved secret **blocks** the write — and blocks the call, at
the gateway. Nothing touches disk without `--write`.

## The adoption ladder (meet the user where they are)

agentstack is adopted in six steps; most projects sit partway up. Before
proposing anything, detect the current step — bare `agentstack` reports the
directory's state and next step, `agentstack doctor` names what's unwired,
`agentstack guard status` shows hook coverage, and the trust state shows in
`tools_search` / `agentstack_doctor`. Then propose the **next** step, not the
whole ladder:

1. **Unify** — no manifest? Propose `agentstack init` (or interactive
   `agentstack setup` for the human) and `apply` to render every CLI.
2. **Verify** — manifest exists but `doctor` complains? Surface its exact fix
   commands.
3. **Guard** — CLIs unwired in `guard status`? Suggest
   `agentstack guard install` (human decision, one command).
4. **Trust** — a cloned repo declares servers that stay inert? Explain the
   review, surface `agentstack trust .`, and stop — never run it yourself.
5. **Scale** — the same skills/servers copied across projects? Propose the
   central library (`lib add`, reference by name) and profiles.
6. **Confine** — sensitive or untrusted work on a machine with Docker?
   Mention `run --sandbox` / `--lockdown` honestly (kernel-enforced, needs
   Docker); don't present the guard or trust gate as a substitute.

Don't push a user up the ladder mid-task — recommend the next step when it
solves the problem at hand, and note the rest only if asked.

## The three artifact modes (recognize which one a project uses)

1. **Static** — `.mcp.json` / `.claude/skills/` exist on disk, gitignored via a
   managed block (it only ever covers files agentstack itself wrote — a
   hand-written `.mcp.json` or `CLAUDE.md` is never hidden from git). Activate
   with `agentstack use <profile> --scope project --write`.
2. **Clean-at-rest** — nothing generated exists between sessions (the manifest
   has an empty `[profiles.off]`). Capabilities appear only during
   `agentstack run <cli> --profile <p>` or between
   `agentstack session start <p> --scope project` and `agentstack session end`.
   A missing `.mcp.json` in such a project is **intentional — do not create one**.
3. **Zero-files / MCP** — the agent pulls skills itself through the `agentstack`
   MCP server. Open a process-local fence with
   `agentstack_lease_open(profile)`, use `agentstack_list_loadable` to browse
   names + descriptions (optional `query` filters by substring over both),
   then `agentstack_load(name, reason)` for the full
   instructions. Loads are fenced to the leased profile and recorded in memory;
   inspect the trail with `agentstack_lease_status`. To preserve the observed
   set, `agentstack_lease_freeze(name)` proposes a manifest profile; tell the
   human to review it and run `agentstack lock`. `agentstack_lease_close` or
   MCP-process exit drops the state without a file restore. MCP servers flow
   through the same profile fence with **no rendered
   files at all** — compact mode
   collapses them behind `tools_search` (search → inspect → call the
   `<server>__<tool>` name); transparent mode (`--transparent`) advertises
   them directly in `tools/list`.

## The trust gate (know where you stand)

In auto-project mode a repo's runtime surface is **gated**: until a human runs
`agentstack trust <dir>`, only control-plane tools work — no servers spawned,
no secrets resolved. You can always tell where you are: `tools_search` says so
and names the exact trust command, and `agentstack_doctor` includes a
`Trust (auto mode):` line.

- **Never run `agentstack trust` yourself.** It is the human consent gate —
  the entire point is that a human reviews what the manifest runs before
  authorizing it. Surface the command and what the manifest declares; stop.
- Trust pins the manifest layers + `agentstack.lock`. If it reports
  "changed", something was edited (often a `git pull`) — tell the human to
  review and re-trust, don't look for a workaround.

## Policy denials (two layers — read the refusal)

A refused call says which rule blocked it:

- `denied by [policy.tools] … (machine policy — ~/.agentstack/agentstack.toml)`
  — the **user's own machine-wide rule**. Nothing in the repo can loosen it;
  do not edit the project manifest to try. Surface it and move on.
- `denied by [policy.tools] <server> = …` (no machine marker) — the **repo's**
  policy. Editing it is a manifest change like any other: propose, human
  applies (and re-trusts).

Denied tools are also invisible to discovery — a tool you can't find may be
firewalled, not missing. `explain <server>` shows both policy layers.

## Locked runs (a frozen capability surface)

If the session was launched with `agentstack run <cli> --locked`, the bridge
serves a **frozen run grant**: the exact server set and policy ceiling that
passed the pre-launch gates, sealed at launch. Refusals that say
*"unavailable under a frozen run grant"* are by design, not breakage:

- Lease transitions, `agentstack_session_start`/`end`/`freeze`, and manifest
  editors (`add_skill`, `add_server`, `add_from`, `create_profile`) are
  refused for the run's duration — the surface cannot be re-derived or
  widened mid-run, and nothing may resolve secrets into native configs.
- Proxied upstream tools, read-only discovery (`list`, `search`, `explain`,
  `diff`, `doctor`, `lease_status`), and trust-gated skill loading work
  normally.
- If the bridge reports the grant itself refused (stale consent, lost trust,
  changed policy), someone edited pinned content mid-run: tell the human to
  review and re-run `agentstack run --locked` — never work around it.
- Want a capability the fence excludes? Propose the manifest/profile change
  in chat; the human applies it and starts a new locked run.

## Commands you'll actually use

```bash
agentstack                       # orientation: CLIs detected, manifest state, next step
agentstack doctor                # verify wiring: adapters, secrets, drift — with exact fixes
agentstack use <profile> --scope project           # dry-run (always safe)
agentstack use <profile> --scope project --write   # activate
agentstack search <query>        # your central library + catalog + official MCP Registry
agentstack add from <id>         # add a found server to the manifest (not applied)
agentstack lib list              # what the central library holds
agentstack lib sync              # commit/pull/push the library across machines (secret gate enforced)
agentstack explain <name>        # provenance, secrets, footprint of a capability
agentstack doctor --ci           # the full trust gate (validation, lock, policy, content scan)
agentstack doctor --deep         # re-scan skills/instructions for hidden-unicode/injection (--json for machine output)
agentstack report calls          # summarize the gateway call log (who called what, outcomes)
agentstack guard status          # which CLIs have the destructive-command hook wired
agentstack guard test <command>  # judge a shell command against guard policy (nonzero on deny)
agentstack report calls          # usage analysis: unused servers, context cost, recommendations
agentstack secret set NAME       # store a secret in the OS keychain
agentstack restore <target>      # undo a write from its pre-write backup
```

## Rules for agents

- **Propose, don't apply.** Edit the manifest (or use `agentstack_add_server`
  over MCP); let a human review and run `apply`/`use` with `--write`. Dry-run
  everything first — output without `--write` is always read-only.
- **Never hand-edit generated files** (`.mcp.json`, `.claude/skills/`
  symlinks, native CLI configs' managed sections). Change the manifest instead.
- **Never write a secret value into the manifest or library** — use `${REF}`
  and tell the user to run `agentstack secret set REF`. `lib sync` enforces
  this with a fail-closed gate (every server field, plus the outgoing
  commits); if it refuses, fix the definition — **never pass
  `--allow-secrets`** unless the user explicitly says so.
- A blocked write ("unresolved secret") is a feature, not an error to work
  around: surface which `${REF}` is missing.
- A command the **host guard** blocks (`✗ blocked` from the pre-tool-use hook)
  is protecting the user from an accident — explain the denial, don't retry
  variants or route around it. `agentstack guard test <command>` reproduces
  the decision outside an agent session.
- To give a project a new skill that exists in the library: add its name to the
  profile's `skills = [...]` list — no file copying.
- A native config key one CLI needs but the manifest schema doesn't model
  (e.g. Codex's `startup_timeout_sec`) goes under that server's per-target
  extras — `[servers.<name>.extra.<adapter-id>]` — not into the native config
  by hand; the adapter passes it through verbatim and it survives `apply`.
- A server whose owning app rewrites its own config entry (a bundled runtime
  the app updates with itself) gets `owner = "<adapter-id>"` — never hand-sync
  its values into the manifest. `apply` then treats the owner's on-disk config
  as the source of truth: it refreshes the manifest and fans the fresh values
  to the other CLIs instead of proposing a downgrade.
- To manage a harness's native executable add-on (pi's TypeScript extensions,
  OpenCode's JS plugins), declare it as `[extensions.<name>]` — a content-pinned
  source (`path`/`git`) plus exactly one `target` adapter. It is the
  highest-risk kind: the code runs inside the harness process at full user
  permission, so an untrusted or drifted project renders zero extension bytes,
  and `run --locked` re-verifies each delivered copy before launch. Run
  `agentstack lock` to pin or accept a change.
