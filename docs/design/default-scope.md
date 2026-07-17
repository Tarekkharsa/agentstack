# Default write scope — follow the manifest's home

**Status: proposed 2026-07-18 · decision: option (a) — context-derived default**

## Problem

`agentstack apply` / `use` / `setup` default to `--scope global` (and say so in
`--help`). But the README quickstart (step 1: `init` → `apply` inside a repo)
and the "Where rendered files live" story ("**Static** (default) — artifacts
sit on disk, kept out of git by a managed `.gitignore` block") read as
**project** scope. On a real device, a user following the quickstart in a repo
writes their repo's servers into machine-global configs (`~/.claude.json`,
`~/.claude/skills/`, …) shared by every project — and no repo artifacts or
managed `.gitignore` block ever appear. For a tool whose core principle is
containment, the default quietly widens a repo's capability set to the whole
machine.

## Options considered

- **(a) Context-derived default** — the effective default scope is **project**
  when the loaded manifest lives in a project, and **global** when the command
  runs against the machine/personal manifest (`~/.agentstack/agentstack.toml`,
  or a relocated `AGENTSTACK_HOME`). `--scope` always overrides.
- **(b) Keep the global default** — rewrite the README quickstart and step-1
  copy to pass `--scope project` explicitly.

## Caller audit (what depends on the global default today)

Every site that resolves the scope default, and what (a) does to it:

| Caller | Today | Under (a) |
|---|---|---|
| `apply` (`commands/apply.rs:105`) | `unwrap_or(Global)` | context default |
| `use` (`commands/use_profile.rs:89,155`) | `unwrap_or(Global)` | context default |
| `setup` (`commands/setup.rs:72`) | `unwrap_or(Global)` | context default |
| `diff` (`commands/diff.rs:31`) | `unwrap_or(Global)` | context default |
| `instructions` (`commands/instructions.rs:17`) | `unwrap_or(Global)` | context default |
| `adopt` (`commands/adopt.rs:28`) | `unwrap_or(Global)` | context default (symmetric with apply/doctor) |
| `restore <adapter>` slot restore (`commands/restore.rs:44`) | `unwrap_or(Global)` | context default (history-entry undo carries its own scope and is unaffected) |
| `run --profile` (`cli.rs` `RunArgs`) | already `default_value_t = Project` | unchanged |
| `session start` (`cli.rs` `SessionCmd::Start`) | already `default_value_t = Project` | unchanged |
| MCP server (`mcp_server.rs::scope_arg`) | already defaults **project** | unchanged |
| Dashboard (`dashboard/server.rs::scope_of{,_query}`) | falls back to Global when the request omits `scope` — but every action handler passes `Some(scope_of(..))` into `ApplyArgs`/`UseArgs`, so the CLI-level default is never consulted | unchanged (frontend sends scope explicitly; fallback noted, not touched) |
| `doctor` drift section (`commands/doctor.rs`) | hardwired `Scope::Global` | see below — must follow, or it nags forever after a project-scope apply |
| `consolidate`, `connect`, `guard install` | explicit `Scope::Global` by design (harvesting global skills dirs, registering the gateway, machine guard) | unchanged |

The surfaces already disagree: the launch paths (`run --profile`,
`session start`) and the zero-files bridge default to **project**, while the
render-out commands default to **global**. (a) makes them agree.

### The `doctor` coupling

`doctor`'s server-drift check renders plans at `Scope::Global` only. If the
render commands default to project in repos, a fresh quickstart would apply at
project scope and doctor would immediately (and forever) report "pending
apply" drift against the untouched global configs — and `doctor --fix` would
*write* them. So doctor's drift scope must be chosen per target:

- **Scopes with state win.** For each target, drift runs at every scope where
  `state.json` records a previous write for this manifest
  (`target_key(id, scope, dir)`). A user who deliberately applies
  `--scope global` from a repo keeps being checked (and fixed) at global —
  their recorded choice of scope is respected, not second-guessed.
- **Fresh setups use the context default.** With no state at either scope, the
  pending-changes check runs at the context default only — so quickstart →
  doctor is clean, and the first `doctor --fix` writes the same scope
  `apply` would.

## Decision — (a)

(b) documents the surprise instead of removing it, and leaves `setup` — the
guided newcomer path — writing repo servers into every project's global
config. (a) is the least-surprise fix and the security-coherent one: a repo's
manifest lands in repo-local artifacts by default; only the machine manifest
(the personal, cross-project layer, seeded by `init --global`) writes global
config by default. No external users exist, so the behavior change is free.

Semantics: `Scope::default_for(manifest_dir)` (in `agentstack-core`) returns
**Global** iff the resolved manifest dir is the machine home
(`~/.agentstack` / `AGENTSTACK_HOME`, compared canonicalized — same rule
`discover_project_base` already uses to keep the machine manifest from being
treated as a project), else **Project**.

Consequences worth naming:

- Machine-layer instruction fragments (`from_user_layer`) only compile at
  global scope, so a repo apply no longer carries them — correct: they belong
  to the machine manifest's own (global-default) apply, and `--scope global`
  still exists.
- Machine guard hooks ride along only on global applies (`apply.rs`); the
  guard is installed by `guard install` / `init --global` directly, so
  project-default applies don't change guard coverage.
- Adapters without a project-scope slot print the existing honest
  "no project scope, skipping" line during a repo apply; `--scope global`
  remains the escape hatch.
- Docs: `--help` texts, README step 1, and the feature-reference "Scopes"
  section updated to state the context-derived default. The "Static (default)"
  story becomes true as written.

## Test

One focused test (`crates/cli/tests/default_scope.rs`): the quickstart flow —
manifest in a repo, `apply --write` with no `--scope` — produces repo-local
artifacts plus the managed `.gitignore` block and leaves `~/.claude.json`
untouched; the same invocation against the machine home manifest still
defaults to global. Existing tests that relied on the old default in temp
*projects* (cross-manifest prune, owned servers, drift hints, content pinning,
restore history) now pass `Some(Scope::Global)` explicitly — they test
global-scope semantics, not the default.
