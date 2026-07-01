# AgentStack Plan: Provider Import → Central Store (the "one command")

Date: 2026-07-01

Status: proposal (design notes, not yet implemented)

Companion to [`central-store.md`](./central-store.md) (the central library this
imports into) and [`portable-agent-runtime-vision.md`](./portable-agent-runtime-vision.md)
(the single-control-plane North Star). Read the central-store plan first — this
extends it from skills to the full provider-import loop.

## TL;DR

One command sweeps every known provider/CLI (`~/.codex`, `~/.claude`, `~/.pi`,
…), imports the **surfaces agentstack understands** (skills, MCP servers, later
hooks/instructions), stores them centrally under `~/.agentstack`, and leaves each
provider reading from that central source through a **generated view or a
symlink** — with a backup taken first and a preview shown before anything is
written.

```
  ~/.codex/         ~/.claude/         ~/.pi/
   config              config             config
      │ import            │ import            │ import
      ▼                   ▼                   ▼
                    ~/.agentstack/         (source of truth)
                      lib/skills/          ← already built (consolidate)
                      lib/servers/         ← Phase 1b (prerequisite)
                      projects/<id>/       ← per-project mapping (optional)
                      provider-views/      ← generated render targets
      ▲                   ▲                   ▲
      │ render            │ render            │ render
   generated           generated          generated
   provider view       provider view      provider view
```

The skills half of this already ships: `consolidate` moves skills into
`~/.agentstack/lib/skills` and replaces the originals with symlinks. This plan
generalizes that pattern to MCP servers (and later hooks/instructions) and wraps
the whole sweep in one preview-first command.

## Non-negotiable safety rules

These are the invariants the whole feature is designed around. They are more
important than any convenience.

1. **Provider folders are never wholesale moved or owned.** `~/.codex`,
   `~/.claude`, `~/.pi`, … hold auth tokens, caches, history, logs, and
   provider-private state. agentstack must never move, delete, or take ownership
   of these folders.
2. **Only known surfaces are managed.** agentstack reads and rewrites exactly the
   surfaces it models per adapter: the skills directory, the MCP-server config
   entries, and (later) hook/instruction entries. Everything else in a provider
   folder is out of scope and untouched.
3. **`~/.agentstack` is the source of truth.** Imported capabilities live
   centrally; the provider copies become derivatives.
4. **Provider configs receive only generated views or symlinks.** A provider's
   managed surface is either a symlink into the central store (skills today) or a
   region rendered from the central source (servers, via `apply`). agentstack
   only changes its own owned entries/regions; a whole-file write is allowed only
   when it merges and preserves all unmanaged content (some render paths rewrite a
   config file atomically while keeping every unmanaged entry intact).
5. **Every mutation is preview-first and backed up.** Dry-run by default; a
   preview lists every path that would change; a backup is written before any
   destructive change (mirrors `consolidate`'s existing
   `~/.agentstack/backups/` + "managed copy created before original removed"
   invariant).
6. **The repo `.agentstack/agentstack.toml` stays the project source of truth.**
   `~/.agentstack` holds the global library; a project manifest **references**
   central items by name (the name-ref resolution built in Phase 1). Provider
   configs are rendered views, never the authority. Per-project mapping under
   `~/.agentstack/projects/<id>/` is an optional convenience, not a replacement
   for the repo manifest.

## The secret-resolution seam (the most important design point)

Server definitions can live centrally, but **`${REF}` secret values must never be
baked into the library.** The library stores references (`${GITHUB_TOKEN}`),
exactly as `agentstack.toml` does today; the actual values resolve **per machine,
at render/gateway time**, through the existing `Resolver` + secret sources
(env / keychain / varlock). This must hold in every direction:

- **Import**: when reading a provider config that contains a literal secret, lift
  it to a `${REF}` before storing centrally (the existing `discover::lift_secrets`
  path). The central store is commit-/share-safe; it never holds plaintext.
- **Central storage**: `lib/servers/<name>.toml` holds `${REF}`s only.
- **Render (provider view)**: `apply` resolves `${REF}`s against this machine's
  secret sources when writing a provider config, and blocks the write if a
  required secret is unresolved (today's `--allow-unresolved` gate).
- **Gateway/runtime**: proxied calls resolve `${REF}`s at call time
  (`Gateway::from_manifest`), never from a stored value.

Design rule: **the library is a definition store, not a secret store.** Secret
resolution is a render/gateway concern and stays there.

## Current state (verified 2026-07-01)

The import/render round-trip already exists in pieces; this feature unifies them.

- **Skills, fully done**: `src/consolidate.rs` sweeps every adapter's skills dir
  (`discover_skills`), moves each into `~/.agentstack/lib/skills` via the library
  insertion path (`commands::lib::add_skill` — index + checksum + provenance),
  symlinks the originals back, backs up real dirs, and is preview-first
  (`--write`). This is the template for everything else here.
- **Server import**: `src/commands/init.rs` reads each detected adapter's MCP
  config (`read_config_value`), extracts servers, and lifts inline secrets to
  `${REF}` (`src/discover/mod.rs` — `merge_servers`, `lift_secrets`). Writes them
  into a *project* manifest today, not a central home.
- **Server render**: `apply` (`src/render/…`) writes manifest servers back into
  each provider's MCP config, resolving `${REF}`s per machine and blocking on
  unresolved secrets. Only agentstack-owned regions are written/pruned.
- **Gateway**: `src/gateway.rs` resolves `${REF}`s at call time for proxied MCP
  tools.
- **Missing**: no central *server* store under `~/.agentstack` (servers are
  per-project only), and no single orchestrator command.

## Phase 1b prerequisite: central server store — COMPLETE (2026-07-01)

The prerequisite is **done**. MCP servers now live centrally and resolve by name,
the exact analog of the skills library — so the orchestrator can be built cleanly.

- ✅ `~/.agentstack/lib/servers/<name>.toml` holds a reusable server definition
  (`${REF}` secrets only), indexed as `[[server]]` in `library.toml`.
- ✅ `resolve_server` resolves `[servers]` / profile server refs by name from the
  central store, inline-first (inline override matches skills).
- ✅ Secrets stay per-machine at render/gateway time; the resolver never resolves
  them, and the lock pins the definition digest only.
- ✅ `apply`/`diff`/`use`/`session` render library servers; `doctor`/`explain`
  report origin/provenance/drift; `lib add-server/list/remove-server` manage them.

The server-specific open questions (in
[`central-store.md`](./central-store.md#phase-1b-design-decisions-resolved-as-built))
are all resolved as built. The **orchestrator (step 3 below) is still proposed.**

## Build order

1. ✅ **Central server store** (Phase 1b). `lib/servers/` + resolver + validation
   + render + lock/drift + `lib` UX. **Complete and reviewed.**
2. **Server consolidate + provider-view render.** The `consolidate`-for-servers
   half: import each provider's MCP entries into `lib/servers/`, then render each
   provider's MCP config as a view from the central source, backup-first and
   preview-first. Reuses `init`'s import path and `apply`'s render path.
3. **The one orchestrator command.** A single preview-first command that runs
   skills-consolidate + server-consolidate + provider-view render + backups in
   one sweep across all detected adapters, with a combined preview and a single
   `--write` to apply. Name TBD (candidates: `import`, `onboard`; not `migrate` —
   `lib migrate` already means the skills-home move).

## Open questions for this feature (beyond Phase 1b)

- **Per-project mapping.** Do we materialize `~/.agentstack/projects/<id>/` at
  all, or rely solely on repo `.agentstack/agentstack.toml` + the global library?
  (Leaning: repo manifest stays authority; central `projects/` only if a real
  need appears — avoid inventing global mutable per-project state prematurely.)
- **Provider-view mechanism per surface.** Symlink (skills) vs rendered region
  (servers) — confirm each adapter surface's mechanism and that pruning stays
  contained to agentstack-owned regions.
- **Scope of the sweep.** Global provider configs (shared `~/.codex` etc.) is the
  first target; project-scoped provider configs are a follow-up. No filesystem
  crawl for repos in v1.
- **Idempotency + re-run.** Same content already central → no-op (reuse the
  library checksum check `consolidate` already does).

## Definition of done (per the vision's bar)

Every phase ships with: CLI command + help text, a docs recipe, dry-run/preview
where files are touched, backups before destructive changes, tests for
success/refusal/idempotency/rollback, `doctor`/`explain` visibility, and clear
ownership rules for every provider surface written or symlinked. No provider
folder is ever moved or deleted; no plaintext secret ever lands in the central
store.

## One-line positioning

> Run one command and every provider you use starts reading its skills and MCP
> servers from a single reviewable `~/.agentstack` — provider folders keep their
> auth and history untouched, receive only generated views or symlinks, and every
> change is previewed and backed up first.

Sources: [`central-store.md`](./central-store.md),
[`portable-agent-runtime-vision.md`](./portable-agent-runtime-vision.md), and the
shipped machinery in `src/consolidate.rs`, `src/commands/init.rs`,
`src/discover/mod.rs`, `src/render/`, `src/gateway.rs`.
