# AgentStack Plan: Central Capability Store

Date: 2026-07-01

Status: Phase 1 complete; Phase 1b central servers complete; hooks remain later

Companion to [`portable-agent-runtime-vision.md`](./portable-agent-runtime-vision.md)
and [`code-mode.md`](./code-mode.md), and the foundation for
[`provider-import.md`](./provider-import.md) (the "one command" that imports every
provider's surfaces into this store). This plan does not replace either; it moves
the **physical home** of capabilities from per-repo to a single central library,
and defines how projects pull from it — declaratively (names in config) and
dynamically (code-mode search/execute).

## TL;DR

Today every capability a project uses is declared **and often stored inside that
project**: `skills/` sits at the repo root, `[skills.*]` paths point at
`./skills/...`, and each repo carries its own `.agentstack/` manifest. A user
who works across many repos ends up with the same skill/server copied into each
one, and no single place to manage them.

This plan makes **one central library the source of truth** for skills, MCP
servers, hooks, and instructions. A project no longer *contains* these; it
**references** them by name (in `agentstack.toml`) or the agent **loads them on
demand** through code-mode (`tools_search` + bindings). Profiles filter the
central library per project/run. This is the "single control plane" from the
product vision made physical.

Rollout order (confirmed with product owner 2026-07-01):

1. **Central home store first** — one library under the agentstack home; projects
   reference it. Start here.
2. **Per-repo overlay second** — repos may still ship local capabilities that
   merge over the central library. Deferred to a later phase, not this one.

Both reference paths ship: an agent can dynamically load what it needs via the
code-mode search/execute surface, **or** a human can name capabilities in the
project's `agentstack.toml`. They compose; neither is exclusive.

## Why this fits the vision

From `portable-agent-runtime-vision.md` — Product Principles:

- "Prefer reproducibility over magic."
- "Prefer explicit previews over silent mutation."
- "Prefer local secret resolution over committed secrets."

And from the auto-memory product vision: *agentstack is the single control plane
— manage skills/MCPs/settings for every CLI without going outside it.* A central
store is the storage-layer expression of that: capabilities live in one managed
place agentstack owns, and every harness/project is a *view* onto it.

The move is safe because the central home already exists in pieces (see Current
state) — we are unifying and formalizing primitives that ship today, not
inventing a new global mutable state surface.

## Current state (verified 2026-07-01)

The agentstack home (`~/.agentstack`, override via `AGENTSTACK_HOME`) already
hosts several managed stores. This plan unifies them under one "central library"
concept rather than building from zero:

- `src/util/paths.rs`
  - `agentstack_home()` → `~/.agentstack` (honors `AGENTSTACK_HOME`).
  - `skills_home()` → `~/.agentstack/skills` — documented as *"the single managed
    home that consolidated skills are moved into (the CLIs then symlink back to
    here)."* **This is already a central skill store.**
  - `user_adapters_dir()` → `~/.agentstack/adapters`, `backups_dir()` →
    `~/.agentstack/backups`.
- `src/consolidate.rs` — `consolidate()` gathers skills scattered across each
  CLI's dir (`~/.codex/skills`, `~/.claude/skills`, …) into `~/.agentstack/skills`
  and replaces each original with a **symlink back to the managed copy**. Safety
  invariant: managed copy created before any original is removed; real dirs
  backed up first. **This is the exact "one place, harnesses view it" mechanic
  we want, already proven for skills.**
- `src/store.rs` — `Store` at `~/.agentstack/store/` fetches and caches
  capability *sources* (git clones, path passthrough) and produces content
  digests for the lockfile. **This is the fetch/cache layer a central library
  needs.**
- `src/manifest/load.rs` — `resolve_manifest_dir(base)` prefers
  `<base>/.agentstack/` then falls back to legacy repo root. Manifest is still
  **per-repo**; there is no notion of a *home* manifest/library index yet.

So three of the four pieces exist (skill home, source store, symlink-view). The
missing piece is a **central library index + name-based resolution** so a project
manifest can say `skills = ["sql-review"]` and have it resolve against the home
library instead of a repo-local path.

## The model

### One library, two resolution paths

```
                 ┌─────────────────────────────────────────┐
                 │  Central library  (~/.agentstack/lib/)    │
                 │  skills/  servers/  hooks/  instructions/ │
                 │  + library.toml  (the index)             │
                 └───────────────┬─────────────────────────┘
                                 │
        ┌────────────────────────┴────────────────────────┐
        │                                                  │
  (A) Declarative                                   (B) Dynamic
  project agentstack.toml                           code-mode runtime
  references items by NAME              agent calls tools_search → bindings
  skills = ["sql-review"]              loads only what a turn needs
        │                                                  │
        └──────────────┬───────────────────────────────────┘
                       │
                 Profile filter (per project / per run)
                       │
                 Rendered into each harness (adapters)
```

- **(A) Declarative — names in `agentstack.toml`.** A project references central
  items by name; the definition lives only in the library. Profiles further
  narrow which referenced items a given run sees. Human-reviewed, reproducible,
  diffable — the default for team-shared setup.
- **(B) Dynamic — code-mode.** The agent discovers and calls capabilities at
  runtime via `tools_search`/bindings ([`code-mode.md`](./code-mode.md)) without
  anything being named in config first. Keeps context small; good for
  exploratory or one-off use. Phase 3 of the code-mode plan already scopes the
  proxied surface to the active profile — the same fence applies here.

The two paths share one physical library and one resolver. Path A is "decide
ahead of time, in git." Path B is "decide in the moment, in the agent." A
capability promoted from B to A is just a name added to `agentstack.toml`.

### Proposed layout

```
~/.agentstack/
  lib/                     # the central library (new)
    library.toml           # index: name → {kind, source, version, digest}
    skills/<name>/         # skill dirs (consolidates today's skills_home())
    servers/<name>.toml    # reusable MCP server definitions
    hooks/<name>.toml      # reusable hook definitions
    instructions/<name>/   # reusable instruction sets
  store/                   # fetched/cached sources (exists today)
  backups/  adapters/      # exist today
```

`skills_home()` (`~/.agentstack/skills`) folds into `lib/skills/` (with a
back-compat shim / migration, mirroring how the repo layout migrated to
`.agentstack/`). `library.toml` is the new index — the home-level analog of a
project manifest, listing what is installed centrally and where it came from.

### Project manifest: reference by name

Today (`agentstack.toml`), a skill is defined inline with a path:

```toml
[skills.figma_repo_handoff]
path = "./skills/figma-repo-handoff"     # repo-local, copied per project
```

Proposed — reference a central item by name (no path, no local copy):

```toml
[profiles.design]
servers = ["figma", "kibana_mcp"]        # already names; now resolve to lib/servers/*
skills  = ["figma_repo_handoff"]         # resolves to lib/skills/figma_repo_handoff
```

Resolution order for a name (first hit wins):

1. **Inline definition** in the project manifest (`[skills.<name>]` with a
   `path`) — explicit local override, always wins. (This is the seed of the
   Phase-2 per-repo overlay.)
2. **Central library** — `lib/library.toml` / `lib/skills/<name>`.
3. **Catalog** — installable but not yet in the library (`agentstack add from`
   fetches into `lib/` first, then references it).

Unresolved name → hard error with a clear message and the `add from` hint, in
keeping with "no silent truncation."

## Phases

### Phase 1: Central **skill** library + resolver + lock/explain (this plan's core)

Deliberately **skills only**. Servers already use name references in profiles and
hooks are lower-value; both are fast-follows (Phase 1b). Framing Phase 1 as
"centralize every capability type at once" is the failure mode — this phase is
"central skill library + resolver + lock/explain," nothing more.

- [ ] Define `lib/` layout and `library.toml` schema for skills (name, source,
      version, digest, provenance). Servers/hooks tables are reserved but empty.
- [ ] Migrate `skills_home()` → `lib/skills/`; keep a back-compat read of the old
      path. Migration is an **explicit `agentstack lib migrate`, preview-first**
      (reuse the `.agentstack/` migration pattern in `manifest/load.rs`).
- [ ] Add a resolver: skill name → library item, with the 3-step order above
      (inline → library → catalog). One central point so both the declarative and
      code-mode paths use it.
- [ ] Extend `[skills.*]` / `profile.skills` to accept a **name-only reference**
      (no `path` body) that resolves against `lib/`.
- [ ] **Reproducibility (the key tightening).** On resolve, record the chosen
      central item's digest/version in the project `agentstack.lock`. The lock
      already carries exactly this: `LockedSkill { name, rev, checksum }` in
      `src/lock.rs`, and `Store::resolve` already returns a `checksum`. Two
      machines resolving `skills = ["sql-review"]` must land on the same content
      or fail loudly — a bare name must never silently resolve to divergent local
      versions.
- [ ] **Drift check in `doctor` / `explain`.** `doctor` flags when the locally
      resolved library digest differs from the `agentstack.lock` entry (central
      item changed under the project's feet). `explain` shows, per skill, whether
      it resolved from inline / library / catalog, the physical source path, and
      the locked digest.
- [ ] `agentstack lib list` / `lib add` / `lib remove` — manage the central skill
      library directly (analogous to today's catalog/pack commands, targeting the
      home library).

Exit criteria:

- [ ] A project `agentstack.toml` that only *names* skills renders correctly for
      every target adapter, pulling bodies from `lib/skills/`.
- [ ] The same central skill referenced by two projects exists on disk once.
- [ ] Resolution order (inline → library → catalog) is tested, including the
      unresolved-name **hard error**.
- [ ] `agentstack.lock` records the resolved central digest; a changed central
      item is flagged by `doctor` as drift, not silently applied.
- [ ] `agentstack lib migrate` moves `~/.agentstack/skills` → `lib/skills`
      reversibly and preview-first; refuses (no-op) if already migrated.

### Implementation pressure points (verified 2026-07-01)

Name resolution touches three places that today assume skills are locally
defined by path:

- `src/manifest/validate.rs:100` — profile skill refs must exist in the local
  `[skills.*]` table (`!manifest.skills.contains_key(kref)` → "unknown skill").
  Must be widened to also accept names resolvable from `lib/` (and keep the
  existing `["*"]` wildcard behavior).
- `src/commands/use_profile.rs` — `resolve_active_skills(...)` resolves active
  skills from manifest-defined paths, then `skills::plan`/`materialize` writes
  them into each target's skills dir. Must resolve name-only refs through the new
  library resolver before materializing.
- `src/consolidate.rs` — already centralizes skill *files* into the managed home
  but still writes project-manifest **path** entries (`[skills.<name>].path`).
  Should instead (or additionally) write **name references** so consolidation
  produces the new central-reference form, not a repo-local path.

### Phase 1b: Centralize servers (complete), then hooks (later)

**Central servers — complete (2026-07-01).** Servers mirror the skills library
end to end: `lib/servers/<name>.toml` definitions indexed as `[[server]]` in
`library.toml`; `resolve_server` (inline-first, then library) returns the
definition with `${REF}`s intact; profile server refs validate against the
library; `apply`/`diff`/`use`/`session` render library servers via one effective
server map (`resolve_active_servers` → `plan_target_with_servers`); the project
lock pins the **definition digest** only; `doctor`/`explain` report origin,
provenance, and definition drift; and `agentstack lib add-server/list/remove-server`
manage them. Secrets resolve only at render/gateway time — never in the resolver
or lock. Gateway unchanged.

- [x] Library + resolver extended to `lib/servers/*.toml` (`[[server]]` index).
- [x] Server refs keep `${REF}` secrets in the library, resolved per-machine at
      render/gateway time (unchanged from today).
- [ ] **Hooks** — the same treatment for `lib/hooks/*.toml`. Not started.

#### Phase 1b design decisions (resolved as built)

- **Reference shape:** name-only ref in `[servers]`/profiles resolves from
  `lib/servers/<name>.toml`; inline `[servers.<name>]` full table still allowed.
- **Inline override:** yes — inline always wins over the library (matches skills).
- **Secret resolution:** render/gateway only; the library stores `${REF}`s and
  the resolver never resolves them.
- **What is locked:** the **definition digest** only (not resolved secrets, not a
  provider-specific render shape). Rendered-shape digest deferred (no model slot).
- **doctor/explain:** origin (inline/library), provenance, and definition-digest
  drift, via `server_lock_status` — the server analog of `skill_lock_status`.

### Phase 2: Per-repo overlay (deferred — the second model)

The product owner asked to *support the first two models but start with the
first*. This phase adds the overlay:

- [ ] A repo may ship local capabilities (`.agentstack/skills/...`) that **merge
      over** the central library — repo item with the same name wins (already the
      resolver's step 1).
- [ ] `agentstack diff` / `explain` show which items are central vs.
      repo-overridden.
- [ ] Deterministic merge with clear precedence; tests for override + fallthrough.

### Phase 3: Dynamic loading via code-mode (integrates the code-mode plan)

- [ ] `tools_search` ranks over the **central** server/tool surface, filtered by
      the active profile (extends code-mode Phase 3's per-server profile fence).
- [ ] Generated bindings target central servers; secrets resolve at call time via
      the gateway (unchanged from code-mode plan).
- [ ] "Promote" flow: a capability an agent used dynamically can be written into
      the project `agentstack.toml` as a named reference (path B → path A) with a
      preview diff — never a silent config write (D20 trust gate).
- [x] **Non-fetching dry-run resolution.** `resolve_skill` takes a
      `ResolveMode::{Fetch, NoFetch}`; `NoFetch` resolves git sources only from an
      existing store clone and reports an un-cached one as
      `ResolveError::NotAvailableOffline` (a non-fatal `SkillLockStatus`). `use`
      dry-run, `validate`, `doctor`, and `explain` all resolve offline; only
      `use --write` (and `lib add --git`) fetch. `doctor`/`explain` dropped their
      git pre-checks and now render the offline case from the status.

## Trust / safety

- **Human-gated writes.** Adding to the central library or naming an item in a
  project manifest is previewed; live harness config is written only on `apply`.
  The agent proposes to the manifest only (existing D20 gate in `mcp_server.rs`).
- **Containment carries over.** The library reuses the ownership/containment and
  atomic-write/backup rules already enforced for consolidated skills and packs
  (`consolidate.rs` safety invariant; pack `safe_skill_dirs`). Nothing outside
  `lib/` managed paths is pruned.
- **Secrets never centralized.** `lib/servers/*.toml` store `${REF}`s, not
  values, exactly like today's `agentstack.toml`. The central store stays
  commit-/share-safe; secrets resolve per-machine at call time.
- **Provenance in the index.** `library.toml` records each item's source and
  digest so a central item is auditable, not an opaque blob.

## Decisions (resolved 2026-07-01)

1. **Library index format → `lib/library.toml`, not directory scanning.** An
   index gives fast `lib list`, provenance, and integrity (digest per item);
   directory-scan loses metadata and can't record where an item came from. The
   index also matches manifest ergonomics developers already know.
2. **Phase 1 scope → skills only.** Lowest risk, directly extends the existing
   `skills_home()` + consolidate machinery. Servers are Phase 1b (they already
   use name refs in profiles), hooks after. Do **not** centralize every
   capability type in the first cut.
3. **Migration → explicit and preview-first.** `agentstack lib migrate` moves
   `~/.agentstack/skills` → `lib/skills`, shows every path first, refuses if
   already migrated. No auto-migrate on run, per "prefer explicit previews over
   silent mutation."
4. **Name collisions → hard error.** If a name resolves in more than one source
   (e.g. central library and catalog) with no inline override, error and list
   both sources rather than silently picking one. Same for an unresolved name.

Reproducibility is folded into Phase 1 rather than left as an option: a
name-only ref (`skills = ["sql-review"]`) records the resolved central digest in
`agentstack.lock`, and `doctor`/`explain` flag drift. See Phase 1.

## Definition of done (per the vision's bar)

Every phase ships with: CLI command + help text, a docs recipe, dry-run where
files are touched, tests for success/refusal/idempotency/rollback, `explain`
visibility, dashboard visibility where it affects live setup, and clear
ownership rules for any files created or moved.

## One-line positioning

> Your skills, servers, hooks, and instructions live once in a central library
> agentstack owns; every project and harness is a filtered view onto it —
> named in reviewed config or loaded on demand by the agent — never re-copied,
> never leaking secrets.

Sources: [`portable-agent-runtime-vision.md`](./portable-agent-runtime-vision.md)
(North Star, home layout), [`code-mode.md`](./code-mode.md) (dynamic
search/execute surface), and the shipped home primitives in `src/util/paths.rs`,
`src/consolidate.rs`, `src/store.rs`.
