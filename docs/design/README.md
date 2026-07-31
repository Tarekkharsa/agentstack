# Design documents — which one answers which question

These explain **active technical contracts**: why a boundary is where it is and
what it guarantees. None of them is a roadmap, and none authorizes work. That
belongs to the four files above them:

| Question | File |
|---|---|
| What is this product for, and what will it refuse to become? | [`STRATEGY.md`](../../STRATEGY.md) |
| What should be worked on next? | [`TODO.md`](../../TODO.md) — the only ordered queue |
| How do I behave in this codebase? | [`CLAUDE.md`](../../CLAUDE.md) |
| What shipped, and when? | [`CHANGELOG.md`](../../CHANGELOG.md) |

Start with `STRATEGY.md` → `TODO.md` → [`ARCHITECTURE.md`](../ARCHITECTURE.md) →
[`ENFORCEMENT.md`](../ENFORCEMENT.md). Come here only when you are changing one
of the boundaries below.

| Document | Answers | Status |
|---|---|---|
| [`strategy-v2-vision.html`](strategy-v2-vision.html) | What the finished v2 product looks like on screen: the 12-moment journey as CLI mockups, t3code panel wireframes, the full command surface, and the feature inventory | Illustrative record for [`STRATEGY.md`](../../STRATEGY.md); the strategy wins on any divergence |
| [`launch-plan.md`](launch-plan.md) | What the completed 2026-07 t3code/workflow prototype built, and why its former launch headline was superseded | Closed implementation record; current launch gate is `TODO.md` |
| [`ui-control-plane.md`](ui-control-plane.md) | How t3code drives AgentStack — the fixed argv actions, the versioned JSON reads, and why the frontend is never an enforcement boundary | Active |
| [`panel-wireframe.md`](panel-wireframe.md) | The popover's daily shape: one card + footer, inline toolset switch, and the three-click mode change with its real plan | Active; awaiting §1.6 study evidence |
| [`workflows-capability.md`](workflows-capability.md) | What a workflow is allowed to be: authoring, authority, and evidence boundaries for the experimental capability kind | Active contract, experimental capability |
| [`workflow-scaling.md`](workflow-scaling.md) | How governed workflows scale without relaxing the capability contract | Active; Phases 0–1 landed |
| [`tools-execute-threat-model.md`](tools-execute-threat-model.md) | What `tools_execute` is and is not defended against | Experimental; [`ENFORCEMENT.md`](../ENFORCEMENT.md#experimental-tools_execute) is authoritative |
| [`adr-tools-execute-runtime.md`](adr-tools-execute-runtime.md) | Why `tools_execute` runs where it does, and who owns it | Accepted decision record |
| [`t3code-mcp-bridge-research.md`](t3code-mcp-bridge-research.md) | Why the MCP harness bridge was **not** built | Closed research — kept so the question is not re-opened from scratch |
| [`reference-field-notes.md`](reference-field-notes.md) | Operational corner cases too deep for [the reference](../reference.md) | Maintainer-facing addenda |
| [`activation-study.md`](activation-study.md) | How to run the §1.6 activation study: recruiting, protocol, metrics, and the Stage 1 gate mapping | Ready to run; results tick the gate in `TODO.md` |
| [`codex-workflow-review-2026-07-29.md`](codex-workflow-review-2026-07-29.md) | What an independent cross-model review found in the workflow interpreter, and the evidence each isolation invariant holds | Closed review record; open findings live in `TODO.md`'s promotion checklist |

## Writing one

A design document earns its place by explaining a **contract that constrains
future code** — a boundary, an invariant, a rejected alternative and why. If
what you are writing is a plan, it belongs in `TODO.md`; if it is a record of
what happened, it belongs in `CHANGELOG.md` or the commit message. Say the
status at the top, and link the authoritative file rather than restating it —
a restated invariant is one that can silently drift from the real one.
