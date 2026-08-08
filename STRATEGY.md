# AgentStack product strategy (v3)

> **Status:** operative product strategy — adopted 2026-08-02 by maintainer
> decision, superseding v2 (adopted 2026-07-31; see git history). Produced by
> the strategy-v3 wayfinder effort; this document stands alone.
>
> **Current as of:** AgentStack 0.18.0-rc.3
>
> **Relationship to the other documents:** [`TODO.md`](TODO.md) is the only
> ordered work queue, re-seeded from "The plan" below. [`CLAUDE.md`](CLAUDE.md)
> carries the binding product-experience and engineering rules.
> [`docs/design/automatic-delivery.md`](docs/design/automatic-delivery.md) is
> the operative delivery contract, amended 2026-08-02 with this adoption. The
> activation-study kit lives at
> [`docs/design/activation-study.md`](docs/design/activation-study.md) as the
> instrument for the bar-met moment. v2 and everything older are history in
> git and `docs/archive/` — never direction.

## The goal

> **Any capability, from anywhere, live in every agent you run — seconds after
> one deliberate yes, and never any other way.**

Identity in one line: **feels like a filesystem, thinks like a vault.**

Every word is load-bearing:

- **Any capability** — the declared kinds (skills, servers, instructions,
  settings, hooks, extensions, workflows, packs) under one mental model. The
  *inert* kinds get the compressed path; the *executable* kinds (hooks,
  extensions) always keep the full ceremony — see "What never changes."
- **From anywhere** — a file the user dropped, a repo they cloned, a
  teammate's bundle, an external registry (including ungated ones). Origin
  stops mattering because review does.
- **Every agent you run** — the portability promise; the thing a
  single-runtime framework structurally cannot say.
- **Seconds** — the authoring bar set by the filesystem-first frameworks, met.
- **One deliberate yes** — consent compressed into a single glanceable,
  content-bound moment. Compressed, never removed.
- **Never any other way** — the invariant that makes the rest worth having.

## The design law

> **Automate everything except the yes.**

Pinning, locking, staging, rendering, drift repair, recovery: all of it is the
system's job, done silently and correctly. The manifest and lock are
system-maintained — written by the machine in the common path, read by humans,
reviewed in pull requests — and the manifest remains the source of truth. The
one thing never automated, inferred, or defaulted is consent to new content.

## The experience contract

The user's entire cognitive surface is four ideas plus one recurring moment:

| Idea | The question it answers |
|---|---|
| **Setup** | What do I have? |
| **Toolset** | What does this task use? |
| **Status** | Is it ready — and if not, what one thing fixes it? |
| **Undo** | How do I take it back? |
| *(the yes)* | *Do I accept this exact content doing these exact things here?* |

Mechanism nouns — manifest, lock, trust digest, adapter, gateway, policy
ceiling — live behind `--explain` and the architecture docs.

**Delivery** is decided, not open: the delivery planner routes each capability
to the dynamic (gateway-lease) or rendered lane by kind and harness — one
automatic behavior, no user-facing modes. **Dynamic is the default at
arc-end:** when the workstreams land (W2 first), gateway delivery becomes the
default for skills and servers on MCP-capable harnesses. Instructions,
settings, hooks, and extensions use injection channels where a harness has
them and rendering where it does not; non-MCP harnesses keep full static
delivery. One escape hatch survives: "render locally", per project or
harness. "Prefer gateway" and clean-at-rest disappear as user-facing
concepts.

## The shape

The end product, settled 2026-08-02. Each decision's full record and
rationale lives in the strategy-v3 effort's feature-list ticket.

- **The library is linked folders — source-agnostic.** Any folder anywhere
  on the device can be linked as a library source, and several at once —
  local skills here, team skills there. Each holds capabilities in the
  clean folder taxonomy (skills/, servers/, instructions/, workflows/,
  extensions/); whether a folder is a git clone, a synced drive, or plain
  local is the user's business — git (`lib sync`) stays the productized
  option for versioning and sharing, never a requirement. Sharing is access
  to a linked folder, solo-first (team features later; signed share/receive
  bundles go quiet until then). Projects select across the linked sources
  and pin exact digests in their lock, so serving stays reproducible no
  matter where content came from; the local store is a cache, never a
  second truth. Authoring is library-first; drop-a-file-in-project survives
  as quick capture. `init` imports existing CLI configs into a linked
  library folder. Name collisions across sources get a precedence rule —
  design-doc work, not strategy.
- **The project is clean.** On MCP-capable harnesses a project carries only
  `.agentstack/` — manifest plus lock, the pinned selection and the consent
  anchor. No CLAUDE.md, no .mcp.json, no .claude/. Non-MCP harnesses render,
  as their only physics.
- **Instructions target CLI and model.** Instruction variants keyed by
  (CLI, model), delivered per harness through the best honest non-project
  channel — MCP `initialize` instructions, global-scope files, flags, hooks.
  The model switch is AgentStack's own orchestration: automatic where the
  harness exposes model identity, explicit toolset switch elsewhere. Status
  states per harness what is actually active; only 6 of 13 adapters carry an
  instruction channel today, and the honesty matrix says so plainly.
- **One card, one yes.** The review stays a single composition card per
  project with one closing yes; its detail body is grouped per capability
  with change markers. Content is reviewed once per project regardless of
  how many CLIs consume it; delivery routing is informational. Per-project
  consent and byte-change re-gating untouched.
- **`run` is protected by default.** Today's `--locked` fail-closed gate
  (trust, strict lock, policy admission, frozen grant) becomes `run`'s
  default; plain host mode is the explicit opt-out; sandbox and lockdown
  stay the isolation opt-ins with their honest posture labels.
- **Varlock is the productized vault.** Already second in the secret chain
  (env → varlock → keychain → .env); `init` offers a `.env.schema`, `doctor`
  checks its health, docs teach it as the recommended vault. `${REF}` and
  fail-closed resolution unchanged.
- **Workflows are a headline capability.** The shipped governed engine
  (sandboxed interpreter, roles, concurrent dispatch, per-step evidence)
  gains per-role `model` and `effort` plumbed through to adapters, named
  algorithm helpers, its open security-review findings closed — then comes
  out of hiding. Right model, right effort, right CLI per role: the thing no
  single-vendor harness can offer.
- **Packaging is self-run materialization.** A toolset and its pinned
  capabilities compose into something you run — a Docker image today,
  your-own-account workers later. Genuinely new build: nothing turns a
  manifest into an image yet. The hosted-runner non-goal stays.
- **The panel is part of the shape.** Review card, library browsing,
  workflow control — extending the existing versioned ui-contract and
  digest-bound action surface. Never a second authority.
- **Everything else is quiet.** Enforcement (policy, egress, recorder),
  guard, undo/history, doctor's checks, analytics, footprint, export/import
  bundles: kept, invisible until a denial or a question makes them speak.

## What never changes

The floor under the plan. None of this is negotiable; the plan relocates
cost, never relaxes guarantees.

- Untrusted repository content is inert. The funnel stages; it never activates.
- All repository content is hostile input: parsed defensively, bounded, never
  interpolated into shell commands.
- No new unsafe code.
- Consent is content-bound. A byte change re-gates — the presentation of
  re-gating improves; the fact of it does not.
- Policy only narrows. Machine ceiling always wins.
- Secrets never serialize. `${REF}` resolves in memory; unresolved fails closed.
- Single authority and dispatch paths, with witnesses.
- Claims match enforcement. Every convenience states honestly what it does
  and does not enforce.
- **Executable kinds — native extensions and hooks — keep the full ceremony.**
  Never a compressed review, in a package or out of one.
- Progressive disclosure must never become progressive enforcement.

## Where we are

The v2 build arc is complete: the file-drop funnel, the review card with
diff-aware re-gates and recognition, the four-ideas surface, seatbelt
legibility with all five denial families recorded, `install --locked`,
signing, and governed external intake all shipped. The code audit behind
this document found the shape's foundations already present too: `lib sync`
already speaks git, Varlock is already in the secret chain, per-adapter
model and effort settings exist, and the workflow engine runs governed
multi-agent work today. What the shape needs is inversion and finish, not
invention — plus the two genuinely unbuilt lanes: automatic delivery (no
lease registry exists in the code) and packaging.

What has never happened: anyone but the maintainer using any of it. This
document's answer: **the product is not yet what its maintainer wants to
hand anyone.** The bar is the maintainer's own satisfaction — the shape
above, finished. Other people — study participants, launch, users — come
after that bar is met, never before.

## The plan

**Structure: a queue plus named revisit triggers. No phases, no gates.**

[`TODO.md`](TODO.md) is the sole sequencing authority. The queue is the
maintainer's to reorder; deviations edit the queue, not this document — the
strategy is reopened only when a named trigger fires, which is what keeps it
from accumulating amendments the way v2's gates did.

The queue as seeded at adoption:

1. **W2 — trust checked at dispatch** — security first: it hardens the
   already-shipped lease path, where a revoked yes can today leave a live
   connection serving until the next load.
2. **The delivery arc** — W1, W3, W5, then W4 (planner, registry, flip)
   last; dynamic becomes the default at arc-end. The clean project lands
   here.
3. **Library inversion** — link-your-folders onboarding (multiple sources),
   library-first authoring, `init` importing existing CLI configs into a
   linked folder.
4. **Instructions** — per-(CLI, model) variants over the injection
   channels, with the per-harness honesty matrix.
5. **Surface finish** — the grouped review card; `run` locked by default;
   Varlock productization.
6. **Workflows promotion** — per-role model/effort, algorithm helpers,
   security findings closed, un-hidden.
7. **Packaging** — toolset into a self-run image.
8. **The panel** — the surfaces above, over the existing ui-contract.
9. **When the bar is met** — re-pin the study kit to the then-current RC,
   run it, fix its three blockers, publish, launch (distribution below).

### Revisit triggers

This document is reopened when — and only when — one of these fires:

1. **The bar met** — the maintainer's declaration that this is a product
   they are happy to hand to other people. Reopens this document to plan the
   study, the release, and the launch under the conditions of that moment.
2. **A competitive tripwire** (see the watch below).
3. **The real-usage threshold** — first sustained external users: issues or
   PRs from strangers, or sustained install growth. This revisit is also
   where ongoing measurement (telemetry versus opt-in studies) gets decided.
   Until then there is no metrics scoreboard: the study kit owns its own
   instruments and baselines.

## The bar

Nothing is put in front of other people — participants, HN, users — until
the maintainer is personally happy with the product. The bar is deliberately
subjective and deliberately first: it replaced an evidence-first release gate
on 2026-08-02, by explicit maintainer decision.

The activation study survives as **the instrument for the moment the bar is
met**: the kit stays ready in `docs/design/activation-study.md`, its
five-threshold pass condition and three-blocker rule unchanged (five
participants using 2+ agent CLIs; ≥4/5 finish unaided; median install→clean
doctor under five minutes; ≥4/5 say "one setup across my CLIs"; 5/5 need no
advanced concepts; ≥4/5 understand every block). Its RC pin is re-cut to the
then-current release candidate when it runs. It no longer gates the delivery
flip — that precondition is amended out of the contract with this adoption.

## Distribution

The launch, once the bar is met and the study has then run:

- **Show HN is the primary moment**, with same-day posts in the Claude Code
  and Codex communities, and a slow-burn presence in the skills ecosystems
  (vercel-labs/skills discussions, awesome-MCP lists, eve's ecosystem)
  positioned as the governed way to use what those registries distribute.
- **The announcement leads with portability** — *"Define your agent setup
  once. Use it across every coding CLI."* — with trust as the second beat.
  Wedge posts may invert the emphasis locally; the launch itself never
  hitches to a competitor's name.
- **Beachhead pair: Claude Code + Codex.** First-run material demonstrates
  this pair and it must be flawless. The audience: developers personally
  running two or more coding CLIs.

## Competitive watch

eve (Vercel) remains not a category competitor: it builds and hosts one new
agent rather than managing the environment of existing CLIs, and it still has
no content trust gate. Its ecosystem remains machine-readable supply for our
governed intake — that prediction shipped.

**2026-08 refresh:** no tripwire has fired. Tripwire 3 is trending: the
de-facto skill distributor today is the sibling project **vercel-labs/skills**
(`npx skills`, 75+ target agents including both beachhead CLIs), which eve's
own docs recommend — the tripwire now names it alongside eve's registry. New
in the same family: major CLIs shipping config-import (Codex `/import`
migrates Cursor and Claude Code configs).

The tripwires — each warrants a strategy revisit, never an automatic build:

1. eve (or a sibling project) imports existing CLI setups.
2. eve renders or exports configuration to other harnesses.
3. eve's registry or vercel-labs/skills becomes de-facto skill distribution
   and users ask us to consume it.
4. A content-bound trust gate ships in that ecosystem.

## Open design questions

Two of v2's three questions closed: where the yes lives in zero-files mode is
answered by [`automatic-delivery.md`](docs/design/automatic-delivery.md), and
`agentstack yes` shipped as a verb. Still genuinely open:

1. **Scoped-MITM credential brokering** — extends what the vault protects,
   not how the yes works. Keeps its own design-doc lane; not part of this
   plan.

## Carried forward (still binding)

- **The promise:** *Define your agent setup once. Use it across every coding
  CLI.* Cross-vendor portability is the product; trust, policy, and evidence
  make it dependable.
- **Surfaces:** the CLI is the primary surface and the sole authority. The
  panel is part of the end shape as the graphical companion over the same
  fixed, digest-bound action contract — never a second authority, never a
  second enforcement boundary.
- **Progressive disclosure:** outcome first, safe defaults silently, blocked
  actions always name the safe next step. Never progressive enforcement.
- **Composition concepts:** manifest, toolset, and the library — the library
  repo is the reuse path across projects; the funnel feeds it, it does not
  replace it. Toolsets are the single selection concept: project selection,
  lease unit, and workflow role are one noun.
- **Engineering strategy:** extend existing seams; never reimplement working
  trust, policy, gateway, runtime, recording, import, render, or restore
  paths; `trust` and `policy` stay small review boundaries.
- **Non-goals:** no agent-building framework or general coding agent, no
  hosted multi-tenant runner, no Cloudflare-specific product (a workers
  deploy *target* among several, on the user's own account, is not that),
  no public marketplace before local reuse earns one, no enterprise
  administration suite, no background-jobs platform, no separate
  repositories for components without independent adoption, and no second
  embedded dashboard. **Retired deliberately from v2's list:** "no generic
  workflow engine" — overtaken; the governed engine is ours, shipped, and
  promoting it is this strategy's decision. "No new capability categories
  before validation" is replaced by the bar: the shape above is the
  committed set, and nothing beyond it starts before the bar is met.

## Adoption record (2026-08-02)

Carried with the adoption, none of them separate decisions:

1. Replace `STRATEGY.md` with this text.
2. Amend `docs/design/automatic-delivery.md`: dynamic default at arc-end;
   the override set reduced to "render locally"; the study precondition
   removed from the flip's list.
3. Promote `docs/archive/design/activation-study.md` to
   `docs/design/activation-study.md`; index it in `docs/design/README.md`.
4. Re-seed `TODO.md` from The plan's queue.

## How this document is used

- `TODO.md` is the queue; this is the map. Before any task starts, the
  admission questions apply: does it serve the shape; is the smallest useful
  outcome defined; could existing features solve it more simply; are success
  and exit criteria measurable.
- Revisit at each named trigger. It should get sharper, not longer.
