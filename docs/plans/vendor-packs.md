# Plan: Vendor Packs

> **Origin.** Distilled from Rhys Sullivan's article *"i don't want to use your agent"*
> (x.com/RhysSullivan/status/2070989582850793947). The article's thesis is the
> demand-side case for agentstack: a vendor should ship its **MCP + skills + docs**
> as portable primitives the user installs into their *own* harness — "one source
> of truth to maintain" — instead of trapping expertise behind an in-app chatbot.
> agentstack is the install target. Today `add`/`search` are server-only; this plan
> adds the missing unit: the **pack**.

## Goal

`agentstack add linear` installs Linear's MCP server **+** skills **+** house-rule
instructions as one unit, rendered into every CLI the user has. `agentstack search`
surfaces packs and standalone skills, not just MCP servers.

## Design principle: compose, don't reinvent

The rails already exist (confirmed by code map):

- `PluginRecipe` (`src/manifest/model.rs:149`) already bundles servers + skills +
  hooks + targets — it becomes our "installed pack" record.
  - **Semantic decision (must settle in Phase 1):** `PluginRecipe` currently exists
    to be rendered by `plugins sync` into each harness's native plugin/marketplace
    shape. A vendor pack is **an install ledger, not a publishable plugin** — it
    records what `add` wrote so `remove` can reverse it. Gate pack recipes out of
    `plugins sync` (e.g. a `kind`/`managed: pack` marker) so installing Linear does
    **not** silently register a harness marketplace plugin. If we ever want packs to
    *also* be syncable plugins, that's an explicit opt-in, not the default.
- Skills already support git/path sources via the store (`src/store.rs`) and
  materialize via `src/render/skills.rs`.
- Instructions already merge per-adapter into `CLAUDE.md`/`AGENTS.md`
  (`src/render/instructions.rs`).

So a **pack is an install-time composition**, not a new runtime concept. After
`add`, each member lives in its normal manifest section and render/apply is
**unchanged**. That is the core leverage of this plan.

```
agentstack add linear
   ├─ [servers.linear]            ← from pack.server  (secrets as ${REF})
   ├─ [skills.linear_breakdown]   ← path/git, fetched + materialized by existing rails
   ├─ [instructions.linear_rules] ← markdown written to ./instructions/, merged per-CLI
   └─ [plugins.linear]            ← recipe recording exactly what this pack added
```

## Decisions (confirmed)

- **Skill source:** bundle starter skills in this repo under `catalog/skills/<vendor>/`,
  referenced by `path`. Self-contained, offline-demoable, deterministic to test.
  The pack model also supports `git`/`rev`, so any pack can migrate to a vendor
  repo later by swapping fields — no machinery change. Starters are clearly labeled
  agentstack-authored, not official vendor content.
- **Scope:** the full rail is Phases 1–9, but ship in two cuts. **MVP = Phases 1–5
  + exactly one pack** (model, `add_pack`, trust gate, discovery, removal) — this
  proves the rail end-to-end. Authoring honest, clearly-unofficial Linear/Cloudflare/
  PostHog skills (Phase 7) is open-ended content work, not a checkbox; treat it as
  ongoing after the rail lands. Upgrade (Phase 6) can follow the MVP **provided
  Phase 1 reserves the lock/recipe fields it needs.**

---

## Phase 1 — Data model & catalog schema

**`src/provider/mod.rs`** — let `Candidate` express more than a server:

```rust
pub enum CandidateKind {
    Server(Install),   // existing behavior, unchanged
    Skill(SkillRef),   // standalone skill (kind: skill catalog entries)
    Pack(PackSpec),    // new: server? + skills + instructions
}

pub struct PackSpec {
    pub server: Option<Install>,     // packs may be skills-only
    pub skills: Vec<SkillRef>,       // name + (path | git+rev) + optional subdir
    pub instructions: Vec<InstrRef>, // name + source (path | url | inline)
    pub targets: Vec<String>,        // default ["*"]
}
```

- Catalog loader: parse `kind: pack` and `kind: skill` entries in
  `catalog/catalog.yaml`. Today `CatalogProvider::search` filters `kind == "server"`
  (`src/provider/mod.rs:142`) — widen to include `pack` and `skill`.
- `PluginRecipe` (`src/manifest/model.rs:149`) currently bundles servers/skills/hooks
  but **not** instructions — add `instructions: Vec<String>`.
- **Reserve for upgrade (Phase 6) now:** the recipe already has `version: String`;
  also record the pack `source` (catalog id / git+rev) so `upgrade <vendor>` can
  re-resolve later. Don't ship `add` with a record that can't be re-resolved.

Catalog entry shape:

```yaml
- kind: pack
  id: linear
  name: linear
  display: Linear
  description: "Linear MCP + ticket-breakdown skill + house rules"
  homepage: https://linear.app
  server:
    type: http
    url: https://mcp.linear.app/mcp
    secret_headers: [Authorization]
  skills:
    - name: linear_breakdown
      path: skills/linear/breakdown      # bundled in catalog/
  instructions:
    - name: linear_rules
      path: instructions/linear/rules.md # bundled in catalog/
  targets: ["*"]
```

## Phase 2 — `add_pack` (the heart)

**`src/commands/add.rs`** — branch `add_from` on `candidate.kind`:

- **Server** → existing path, untouched.
- **Pack** → new `add_pack`:
  1. Write `[servers.<name>]` (if present) via existing `to_server()` + `write_manifest`.
  2. Write each `[skills.<name>]` (path or git/rev).
  3. For each instruction: resolve source → write markdown to
     `<manifest_dir>/instructions/<name>.md` → write `[instructions.<name>] path = "..."`.
  4. Write `[plugins.<vendor>]` recipe listing every added member.
  5. If the server declares `secret_headers`/`secret_env`, write `${REF}`
     placeholders (never literals) and print the exact next step to supply them
     (varlock/env), tying into the existing `explain`/secret-source machinery so
     `doctor` doesn't just fail opaquely. The article's premise is *the user brings
     their own credentials* — make that the install's closing instruction.
  6. Print summary + "run `agentstack apply`."
- **`write_from_provider` is NOT free for packs.** The existing shim
  (`src/commands/add.rs:154`) returns a single capability name and bails if one
  server exists — it structurally assumes one server. Pack install writes 1 server +
  N skills + M instructions + a recipe. Generalize its return (e.g. `Vec<String>` or
  an `Added { servers, skills, instructions }` struct) **or** add a sibling
  `write_pack_from_provider`, so the **dashboard** and **MCP server** reach pack
  install through the same door the CLI does. Plan for this work; don't assume it.
- **Collision rule:** members are vendor-prefixed (`linear_breakdown`); bail with a
  clear pointer if any target name already exists in the manifest.

## Phase 3 — Trust gate & safe instructions (the part that makes packs safe to exist)

Installing a pack pulls **third-party code (skills), a process that runs code (MCP),
and prose that steers the agent (instructions)** into a harness that can see the
user's local files. This is the exact threat the README's trust gate exists for, so
packs must go *through* it — a badge (Phase 4) is not a gate.

- **Policy enforcement at install.** Before writing anything, evaluate the pack's
  members against `[policy]` (`require`/`forbid`/`allowed_sources`). If a member
  violates policy, refuse the whole pack (atomic — no half-installed vendor) with a
  pointer to the offending member and the rule. Mirror what `doctor --ci` already
  enforces so install and audit agree.
- **`doctor` sees pack members.** A pack's server/skills/instructions must surface in
  `doctor` (drift, secrets, connectivity) like any hand-added capability — they ride
  the normal manifest sections, so this should mostly fall out, but add a test.
- **Instructions are a prompt-injection vector — treat them as such.** Vendor markdown
  merged into `CLAUDE.md`/`AGENTS.md` is now steering the user's daily-driver agent.
  Do **not** silently fold vendor prose into house rules:
  - Namespace it visibly (clear `# vendor: linear (unofficial)` provenance header).
  - Show the instruction body as a **diff/preview and require confirmation** before it
    lands in the merged house-rules file (or gate behind an explicit `--with-instructions`).
  - Provenance must survive into the merged output so a user reading their house rules
    can see which lines came from which vendor.

## Phase 4 — Discovery

**`src/commands/search.rs`**:

- `[pack]` badge + "contains: 1 server · 2 skills · 1 instruction" line.
- Aggregate trust signals across members (`runs code` if any stdio member;
  `needs secret` if any member needs one).
- `kind: skill` entries get a `[skill]` badge + git/path source.
- MCP Registry stays server-only (it has nothing else); packs/skills are catalog-sourced.

## Phase 5 — Clean removal

**`src/commands/remove.rs`** — `agentstack remove linear`:

- If the name matches a `[plugins.*]` recipe: remove all members
  (servers/skills/instructions/hooks), delete the fetched instruction files,
  then drop the recipe.
- Otherwise fall back to single-capability removal (existing behavior).

## Phase 6 — Update / upgrade (lifecycle)

`add` and `remove` aren't enough: packs are **versioned vendor content** — a new
skill rev, a changed MCP URL, an added instruction. There must be an
`agentstack upgrade <vendor>` story, and it must interact with the **reproducible
lockfile** the README already promises:

- Record the resolved pack version (skill `rev`, server URL, instruction hashes) in
  the lock at install time.
- `upgrade <vendor>` re-resolves from the catalog/source, shows a diff of what
  changes (especially instruction-body changes → re-confirm, since they steer the
  agent), and re-pins the lock.
- *Scope note:* full upgrade may land after the MVP (see Sequencing). If so, **name
  it here as a known gap** and ensure Phase 1's recipe/lock fields don't foreclose it
  — don't ship `add` with a shape that can't be upgraded later.

## Phase 7 — Seed real packs

**`catalog/catalog.yaml` + `catalog/skills/<vendor>/` + `catalog/instructions/<vendor>/`**

Seed the article's exact demo set: **Linear, Cloudflare, PostHog**, plus one
skills-only example. Real remote MCP URLs; agentstack-authored starter skills
(e.g. Linear "break a big issue into shippable tickets", Cloudflare "product
surface + CLI cheatsheet", PostHog "data-to-query + growth"). Starters labeled
as unofficial.

## Phase 8 — Docs / positioning

**`README.md`** — add the supply-side framing the article supplies:
"agentstack is how a vendor ships its MCP + skills + docs into any harness — one
source of truth." Document `agentstack add <vendor>` as the canonical target for
the article's "continue in your own agent? install the mcp/cli and skills" prompt.

## Phase 9 — Tests

**`tests/`**:

- `add_pack` writes all four sections correctly (and the `[plugins.*]` recipe).
- Pack and skill appear in `search` output with correct badges.
- **A pack that violates `[policy]` is refused atomically** — nothing written.
- **Pack members surface in `doctor`** (secrets/drift/connectivity).
- **Instruction merge carries vendor provenance** and respects the confirmation gate.
- `remove <pack>` fully reverses install, including instruction files.
- End-to-end `apply` after a pack install: renders the server config, materializes
  skills, merges the instruction fragment into each adapter's instructions file.

---

## What this plan deliberately does NOT do

- **No harness.** The article lists a harness as a primitive, but agentstack
  *configures* harnesses; staying harness-agnostic is the stronger position.
- **No model routing / token pass-through** (the Mercury reply idea) — a vendor
  billing concern, out of scope for a config control plane.
- **No render/apply core changes.** Packs ride the existing server/skill/instruction
  rails; the adapter descriptors are untouched.

## Follow-ups from the competitive review

See [competitive-landscape.md](../competitive-landscape.md). Microsoft APM already
ships the mature version of this idea (transitive deps, pack/publish, content
scanning, marketplaces). These items keep vendor-packs from looking like a "syncer"
next to a real package manager — sequence them after the MVP rail lands:

- **Transitive pack dependencies + install-from-any-git-host.** A pack should be
  able to depend on other packs, resolved from any git host (not just the catalog),
  and pinned in the lockfile. This is APM's core power feature and the single
  biggest thing separating "package manager" from "config sync." Make sure Phase 1's
  pack/recipe model doesn't foreclose a `dependencies:` edge.
- **Content-security scan at install (pairs with Phase 3).** APM scans every package
  for hidden-Unicode / prompt-injection *before* the agent reads it; we currently
  only *gate* on policy. Add a scanner + an `audit` command so the trust gate has
  teeth, not just an allowlist — especially since packs carry vendor-authored
  instructions that steer the agent.
- **`pack` / `publish` interop.** When we build the publish side, emit a standard
  `plugin.json` alongside our native format so packs install into the Copilot/Claude
  plugin ecosystems too — interop instead of a walled format.
- **SBOM from the lockfile.** Once packs are lockfile-pinned, `lock export
  --format cyclonedx|spdx` is nearly free and is a real enterprise unlock.

## Risks / open points

- **Instruction file location** (`<manifest_dir>/instructions/`) becomes a managed
  area; `remove` must clean it without touching user-authored files there. Mark
  pack-written files (e.g. a header comment or sidecar) so removal is safe.
- **Catalog asset bundling**: skills/instructions under `catalog/` must be embedded
  with the binary the same way `catalog.yaml` is, or resolved relative to the
  install. Confirm the embed mechanism in Phase 1.
- **Starter-skill accuracy**: authored, not official — keep them honest and clearly
  labeled to avoid implying vendor endorsement.

## Sequencing

Phases 1–2 are ~50% of the work (the model + `add_pack`). Phase 3 (trust gate +
safe instructions) is the safety spine and gates the MVP — not optional. Phases 4–5
are mechanical once the model lands. **MVP ships at 1–5 + one pack.** Phase 6
(upgrade) and 7 (seed real packs) follow; 8 is docs; Phase 9 tests run throughout.
