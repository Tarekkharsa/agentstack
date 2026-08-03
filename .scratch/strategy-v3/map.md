# Map: Strategy v3 — the fresh product definition

Label: wayfinder:map

## Destination

An adopted `STRATEGY.md` v3 — thesis kept ("any capability, from anywhere, one deliberate yes"), plan rebuilt around adoption evidence — replacing v2 as the operative strategy, with `TODO.md` re-seeded from it. Shipping v0.18.0 is the plan's first concrete step, but its execution is outside this map.

## Notes

- Ground truth: the shipped code, `STRATEGY.md` v2 (last adopted reference), and `docs/design/automatic-delivery.md`. `docs/archive/` and git history are history, never direction.
- Decisions locked at charting (2026-08-02): destination artifact is an adopted v3 replacing v2; v2's thesis and invariants stand, the plan is open; the plan orients around evidence-first ("ship to get it"); the map ends at adoption + TODO re-seed. Amended later 2026-08-02: the plan is craftsman-first — finish automatic delivery until the maintainer is happy; study, launch, and other people come after that bar.
- Skills: /grilling and /domain-modeling for grilling tickets; /research subagents for research tickets; /prototype for the draft.
- Tracker: local markdown (this directory). Tickets in `issues/`, `Blocked by:` lines, `Status: open/claimed/resolved`.

## Decisions so far

<!-- one line per closed ticket: [title](issues/NN-slug.md) — gist -->

- [Who is the evidence from?](issues/01-evidence-target.md) — multi-CLI solo devs; beachhead pair Claude Code + Codex.
- [Carry-forward audit of v2](issues/02-carry-forward-audit.md) — most of v2 (thesis, invariants, competitive watch, carried-forward-from-v1) survives; the five-phase plan narrative is done and drops, D1-D7/open-Q1/Phase-3-egress-recording text is stale versus shipped code and `automatic-delivery.md`.
- [Competitive watch refresh: vercel/eve](issues/03-eve-watch-refresh.md) — no tripwire fully fired; tripwire 3 (registry as de-facto distribution) is trending via the sibling vercel-labs/skills project, worth a v3 wording check.
- [Fate of the activation study](issues/04-activation-study-fate.md) — re-adopted as the instrument; amended: deferred behind the maintainer-happy bar, still precondition 8 of the flip.
- [What replaces phases and gates](issues/05-plan-control-structure.md) — queue + named revisit triggers (study result, competitive tripwires, real-usage threshold); no phases, no gates.
- [Where the automatic-delivery arc sits](issues/06-automatic-delivery-arc.md) — amended: the arc is the queue — W2 first, contract order after, flip behind its eight preconditions.
- [Distribution: how users arrive](issues/07-distribution.md) — Show HN primary, CLI communities same-day, skills wedge slow-burn; portability leads; amended: timing follows the maintainer-happy bar.
- [Metrics scoreboard in v3](issues/10-metrics-scoreboard.md) — no scoreboard; the study kit owns the metrics, ongoing measurement deferred to the real-usage revisit.
- [Dynamic-first depth](issues/11-dynamic-first-depth.md) — dynamic default at arc-end; no user-facing modes; one "render locally" escape hatch; rendered lane stays for what MCP cannot carry; study precondition amended out of the flip.
- [Dynamic instructions: what can each harness accept at session start?](issues/12-dynamic-instructions-feasibility.md) — Claude Code is closest to zero-project-files (MCP `instructions` field already confirmed live via AgentStack's own gateway); every harness has at least one global/env/flag channel, but MCP-`instructions` consumption elsewhere is unconfirmed and no harness natively conditions instructions by model.
- [The feature list](issues/13-feature-list.md) — end shape settled: library-repo model, clean project, per-model instructions, content-once review, promoted workflows, self-run packaging, panel in the shape.
- [Draft v3](issues/08-draft-v3.md) — accepted after five rounds; the draft (assets/draft-strategy-v3.md) carries the bar, the settled shape, the seeded queue, and the four adoption follow-throughs.
- [Adopt v3 and re-seed TODO.md](issues/09-adopt-and-reseed.md) — adopted: STRATEGY.md v3 operative, delivery contract amended, study kit promoted, TODO.md re-seeded. Destination reached.

## Not yet specified

## Out of scope

- Executing the release: running the activation study and publishing v0.18.0. First item on the re-seeded queue, not a step on this map.
- Building the automatic-delivery workstreams (W1–W5). The map decides where the arc *sits* in the plan; the build authorizes through the new `TODO.md`.
- Any new capability lane. Standing constraint from `CLAUDE.md`, unchanged by the reset.
