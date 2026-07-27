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
| [`launch-plan.md`](launch-plan.md) | What has to be true before a public launch, and what ships behind it | Self-contained; read alone for launch work |
| [`ui-control-plane.md`](ui-control-plane.md) | How t3code drives AgentStack — the fixed argv actions, the versioned JSON reads, and why the frontend is never an enforcement boundary | Active |
| [`workflows-capability.md`](workflows-capability.md) | What a workflow is allowed to be: authoring, authority, and evidence boundaries for the experimental capability kind | Active contract, experimental capability |
| [`workflow-scaling.md`](workflow-scaling.md) | How governed workflows scale without relaxing the capability contract | Active; Phases 0–1 landed |
| [`tools-execute-threat-model.md`](tools-execute-threat-model.md) | What `tools_execute` is and is not defended against | Experimental; [`ENFORCEMENT.md`](../ENFORCEMENT.md#experimental-tools_execute) is authoritative |
| [`adr-tools-execute-runtime.md`](adr-tools-execute-runtime.md) | Why `tools_execute` runs where it does, and who owns it | Accepted decision record |
| [`t3code-mcp-bridge-research.md`](t3code-mcp-bridge-research.md) | Why the MCP harness bridge was **not** built | Closed research — kept so the question is not re-opened from scratch |
| [`reference-field-notes.md`](reference-field-notes.md) | Operational corner cases too deep for [the reference](../reference.md) | Maintainer-facing addenda |

## Writing one

A design document earns its place by explaining a **contract that constrains
future code** — a boundary, an invariant, a rejected alternative and why. If
what you are writing is a plan, it belongs in `TODO.md`; if it is a record of
what happened, it belongs in `CHANGELOG.md` or the commit message. Say the
status at the top, and link the authoritative file rather than restating it —
a restated invariant is one that can silently drift from the real one.
