# The feature list

Type: grilling
Status: resolved

## Question

The maintainer describes every feature they envision for the product — the full dump, in their own words — and each is discussed for feasibility and cost, one at a time. Known threads going in: per-CLI/per-model instruction control; a clean project with no materialized files; the central library as a user-owned, versioned, shareable repo of skills/tools/connectors/instructions; a much lighter review UX; eve's folder taxonomy as the cleanliness bar. Each discussed feature graduates to its own decision or dies here; the outcomes reshape the draft.

## Discussion record (running)

Maintainer answers so far, 2026-08-02:

- **Enforcement layer** (policy, egress, recorder): quiet infrastructure, loud on denial — never a surface users operate up front; met only in the seatbelt moment.
- **t3code panel: part of the end shape** — supersedes the draft's "optional companion, revisit later" framing; the panel hosts review card, library browsing, workflow control.
- **Authoring: library-first, funnel stays** — the central library repo is the default authoring home; drop-a-file-in-project survives as quick capture that promotes into the library; both end in the same yes.
- **Onboarding: import into the library** — first run scans existing CLI configs (.mcp.json, CLAUDE.md, skills) and offers to move them into the central library.
- Later threads added to the list: deployable agent packaging (Docker now, workers later), toolsets/profiles reshape, cross-CLI orchestrated workflows (per-step model/effort/CLI/instructions + chosen algorithm).
- Research input: [Dynamic instructions feasibility](12-dynamic-instructions-feasibility.md) resolved — instructions ARE partially injectable per harness (Claude Code strongest, confirmed live); per-model conditioning exists natively nowhere and would be AgentStack's own orchestration.

- **Per-CLI/per-model instructions: committed, honest per harness.** Library instructions declare variants keyed by CLI and model; the planner delivers each through the best channel the harness has (MCP `initialize` instructions, global-scope files, flags, hooks — never project files). The model switch is AgentStack's own orchestration: automatic where the harness exposes model identity (Claude Code today; anything run under `agentstack run`), explicit toolset switch everywhere else. Status states per harness what is actually active — claims match delivery.

- **The central library is a user-owned git repo, synced.** Clean folder taxonomy (skills/, servers/, instructions/, workflows/, extensions/); agentstack clones and syncs it per machine; versioning is git; sharing is repo access (teams fork or share one). Projects select from it and pin exact digests in their lock — the shipped pack rail generalized into THE model. The local store becomes a cache/checkout, never a second truth. This also resolves the taxonomy thread (the eve-like organization lives in the library repo).

- **Clean project, end state:** on MCP-capable harnesses the project contains ONLY `.agentstack/` (manifest + lock, committed — the pinned selection from the library and the consent anchor). No CLAUDE.md, no .mcp.json, no .claude/. Instructions travel via injection channels; non-MCP harnesses still render as their only physics.
- **Review scope: content, once per project.** One card per capability — what it runs, reaches, resolves — regardless of how many CLIs consume it; delivery routing is informational, never reviewed. Per-project consent and byte-change re-gating untouched; library-level blanket consent explicitly rejected (context binding kept).

## Answer

Resolved 2026-08-02. The full feature list was discussed one item at a time; the running record above holds each decision with its reasoning. The end-product shape in one view:

- **Central library:** a user-owned git repo (skills/, servers/, instructions/, workflows/, extensions/), synced per machine; projects pin digests from it; the local store is a cache, never a second truth. Authoring is library-first with the project file-drop kept as quick capture; onboarding imports existing CLI configs into the library.
- **Clean project:** `.agentstack/` (manifest + lock) is the only project content on MCP-capable harnesses; injection channels carry instructions; non-MCP harnesses render as their only physics.
- **Instructions:** per-CLI and per-model variants, delivered per harness through the best honest non-project channel; the model switch is AgentStack's own orchestration; status states per harness what is active.
- **Review:** one content card per capability, once per project; delivery routing informational; library-level blanket consent rejected (context binding kept).
- **Enforcement:** quiet infrastructure, loud on denial.
- **Toolsets:** the single selection concept — project selection, lease unit, and workflow role are one noun; exact surface shape is design-doc work, not strategy.
- **Workflows:** promoted to a headline capability — extend roles with model + effort and plumb them to adapters, add named algorithm helpers, close the open security-review findings, un-hide; sequenced after the delivery arc; the "no generic workflow engine" non-goal is retired as overtaken by our own shipped, governed engine.
- **Packaging:** self-run materialization (Docker now, own-account workers later); the hosted-runner non-goal stays.
- **Panel:** part of the end shape (review card, library browsing, workflow control).

These outcomes rebuild the draft ([Draft v3](08-draft-v3.md)).

## Addendum: code-grounded round (2026-08-02)

A four-way code audit (CLI surface + library, adapters, runtime/gateway, trust/panel) corrected assumptions and settled five more decisions:

**Audit corrections recorded:** `lib sync` already implements the git-repo library (init/clone/commit/pull/push with a secret-leak push gate); projects already resolve inline-first → library-fallback and pin content SHA-256s in the lock; per-adapter model/effort settings exist (claude-code, codex, pi) so the workflows gap is per-role plumbing only; the shipped review is ONE composition card per project (not per-capability); packaging has no existing image-build path (`run --sandbox` consumes a hand-built image); only 6 of 13 adapters carry instruction files, so the instructions honesty matrix will have real "not deliverable" cells; Varlock ships today as an opt-in resolver in the secret chain (env → varlock → keychain → .env).

**Decisions from this round:**

- **Sharing: solo-first.** The productized flow now: create a git repo, clone it, link it as the library (`lib sync --init --remote` made first-class in onboarding). Team features come later. Signed share/receive bundles go quiet — kept, not removed — until teams arrive.
- **Run: locked by default.** `run` adopts today's `--locked` fail-closed semantics (trust + strict lock + policy admission + frozen grant) as its default; plain host mode becomes the explicit opt-out; `--sandbox`/`--lockdown` stay the isolation opt-ins with their honest posture labels.
- **Review: one card, grouped inside.** Keep the single composition card and single closing yes — lighter than N cards — and restructure its detail body per capability with change markers. Digest, staging, and standing answers untouched. (Refines this record's earlier "one card per capability" wording.)
- **Varlock: productized as the recommended vault.** `init` detects/offers a `.env.schema`, `doctor` checks varlock health and names it in secret diagnostics, docs teach it as the recommended vault; the chain and keychain fallback stay unchanged.
- **Unmapped shipped features default to keep-quiet:** code-mode executor (experimental), guard, undo/history ledger, doctor's check categories, usage analytics, footprint, age-encrypted export/import bundles — all stay as quiet infrastructure unless a later decision says otherwise.

## Refinement (2026-08-02, draft review)

**The library is source-agnostic linked folders, not one git repo.** Any folder anywhere on the device links as a library source; several can be linked at once (local skills, team skills). Git via `lib sync` stays the productized versioning/sharing option for a linked folder, never a requirement. Projects pin content digests across all sources, so serving stays reproducible regardless of origin. Open design-doc item (not strategy): the name-collision/precedence rule across sources.
