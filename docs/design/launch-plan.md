# Launch plan — the single source for the launch effort

> **Read only this file for the launch work.** It is self-contained: the launch
> bar, current state, findings, agreed decisions, the reviewable-workflow design,
> and the ordered to-do list all live here. Do not go hunting through other
> design docs for this effort — everything needed is below.
>
> **Status:** design agreed 2026-07-23. Prototype-scoped for v1. When a lane is
> accepted for build, its tasks move into the checklist at the bottom and, if it
> touches a security boundary, gets an explicit go before entering `TODO.md`.

---

## 1. The launch bar

Public launch is gated on the **closed loop working end-to-end from t3code's
UI** — the everyday journey never forces a drop to the CLI:

1. **Doctor** — run "check setup", see status + one next action.
2. **Onboarding/init** — from a fresh machine (no `agentstack` installed):
   install → run init in the UI → choose the setup mode / grant the allows →
   apply, all in the UI.
3. **Skills & profiles (toolsets)** — browse the library, **load/add** skills &
   servers into a toolset, **create** a new toolset, and activate it — from the UI.
4. **Reviewable workflow** — when the model proposes a workflow, the UI shows its
   **shape** as a graph (phases, agents, each agent's model / effort /
   instruction) and the user **approves / rejects / edits-with-the-model** before
   it runs.

Everything else (Stage 4 sharing, advanced enforcement UI, native declarative
workflow execution) stays behind launch.

---

## 2. Where we already are (do not rebuild)

Grounded in the current `agentstack-panel` branch of t3code and the CLI's
`ui_contract` surface:

| Loop item | Engine / CLI | t3code UI | Verdict |
|---|---|---|---|
| Doctor | `status-v1` (`doctor --json`) | Overview panel *is* the doctor | **Done** |
| Onboarding | `init-plan` + `apply-setup`, digest-bound, `--secrets env\|keychain\|skip` | SetupPanel render→apply | **~Done, 2 gaps (Lane A)** |
| Trust / drift / undo | `trust-preview`, `diff-v1`, `restore-last` | Trust + drift + undo panels | **Done** |
| Sessions | `profiles-v1`/`sessions-v1` (`use --list --json`) | "Use temporarily" in ToolsetsCard | **Done** |
| Skills/profiles | mutation logic exists **only as agent-facing MCP tools** | view + activate only | **Gap (Lane B)** |
| Workflow | imperative script; **no declarative plan / per-agent model/effort/instruction / live stream** | observe-only monitor (polls) | **Gap (Lanes C1, C2)** |

---

## 3. Structural constraints (why the work takes the shape it does)

1. **The panel bridge is CLI-argv, not MCP.** t3code drives AgentStack through a
   closed set of fixed CLI actions (pinned by `crates/cli/tests/t3code_parity.rs`),
   with reads as versioned JSON (`crate::ui_contract::envelope`, `schema_version`
   + `features`). The `agentstack_*` **MCP tools are a separate, agent-facing
   plane** (for Claude Code / Codex inside a session). **New UI capability = new
   fixed argv actions**, not wiring MCP into the browser.
2. **House rule for every skill/profile write:** mutate manifest → **re-lock**
   (`use --write`) → **re-render** → digest-bound consent. New actions wrap that
   pipeline; they never edit rendered output.
3. **The workflow engine is imperative.** A workflow is a Boa JS script; shape is
   encoded in control flow (`agent`, `parallel`, `pipeline`, loops). `meta`
   carries only `{roles, max_agents, max_wall_seconds}`; `phases` are parsed then
   discarded. **Model comes from the role's profile; effort is unmodeled; the
   prompt is hashed** into `wf_request_digest`, not stored as text. Run reports
   carry `step/role/label/state/timing` — no model/effort/instruction. There is
   **no pre-run plan/dry-run**. → An editable pre-run plan is net-new; v1 avoids
   the engine (§6).

---

## 4. The lanes

### Lane A — finish onboarding (small, UI-weighted)

The render→apply loop exists; close the "fresh machine" edges:
- **A1** — detect `agentstack` absent (PATH / `T3CODE_AGENTSTACK_BIN` miss) and
  show install guidance instead of a dead panel. UI-only.
- **A2** — surface the setup mode / allow choice (the CLI already takes
  `--secrets env|keychain|skip`); add the picker to SetupPanel. UI-first;
  confirm no new CLI surface needed.

### Lane B — skills & profiles: load + activate + create (medium)

Depth agreed: **load + activate + create; no inline field-editor for launch.**
- **B1** — new fixed CLI actions (enveloped + digest-bound) wrapping the existing
  mutation logic behind the MCP tools (`agentstack_add_skill`, `add_server`,
  `create_profile`): add-skill-to-profile, add-server-to-profile, create-profile,
  use-profile. Each runs manifest → **re-lock** → **re-render**, returns a consent
  digest.
- **B2** — extend the versioned contract: a `features` entry (e.g.
  `profiles-edit-v1`) + a panel-argv read for the loadable library index (reuse
  `agentstack_list_loadable`'s data behind a read, not the MCP tool).
- **B3** — extend `t3code_parity.rs` `dispatch()` for the new verbs; witnesses:
  create/add re-locks + re-renders and fails closed on unresolved `${REF}`.
- **B4** — t3code UI: library browser + "add to toolset" + "new toolset", on the
  existing ToolsetsCard system + confirm/consent-digest pattern.

### Lane C1 — workflow observe contract (small)

Wrap `workflow list/runs` in `ui_contract::envelope` + a `features` entry (e.g.
`workflow-observe-v1`), making the existing monitor a stable negotiated contract.
No engine change.

### Lane C2 — reviewable workflows, v1 (medium; **engine untouched**)

The headline. Full design in §5–§6 below.

---

## 5. Reviewable workflows — what & why

When a user asks the model for a workflow, the model picks the **best shape for
the task** — MapReduce, a pipeline, a tournament/judge-panel, loop-until-dry, a
DAG — and the user reviews that **shape as a graph** before it runs, then
**approves / rejects / edits-with-the-model**.

The value is **seeing which *algorithm* the model chose** — the shape is itself
judgment worth reviewing (fan-out map→reduce for "audit this repo" vs a judge
panel for "design an API"). That is the workflow launch headline.

**Agreed decisions:**
- **Mode is the user's intent, not a setting.** "Just run a workflow" vs "build a
  workflow and let's review it" are different requests; the model infers which
  and routes.
- **One pipeline, one gate.** Every run produces a plan; the gate auto-skips
  ("just do it") or pauses ("review together"). Two entry points: **propose**
  (emit plan, don't execute) and **run**. The default is a one-line policy,
  switchable later — not worth agonizing over now.
- **Review shows topology, not a flat agent list** — fan-out, fan-in, chains,
  loops, gates — and names the pattern.
- **Hybrid: model emits *structured data*, a thin layer renders it.** A **skill**
  makes the model output a **blueprint** (topology + per-node settings + pattern +
  symbolic fan-out); a **thin deterministic renderer** draws it. The model does
  *not* paint pixels — a picture is inert; a blueprint is a control surface you
  approve/edit/run against.
- **Three actions:** **Approve** (run as shown) · **Reject** (cancel) · **Edit
  with the model** (conversational — user says what to change, model re-emits the
  blueprint, graph re-renders; loop until approve). No manual node-dragging in v1.
- **Agent-as-compiler shortcut (why v1 needs zero engine change):** on approve,
  the model generates the runnable workflow from the approved blueprint and runs
  it via existing `workflow run`.

---

## 6. Reviewable workflows — the design

### The blueprint (source of truth for the graph and for "approve")

The model emits this as structured JSON for a workflow it authored.

```jsonc
{
  "workflow": "repo-audit",
  "pattern": "map-reduce",        // map-reduce | pipeline | tournament |
                                  // loop-until-dry | dag | custom
  "goal": "Find and rank bugs across the changed files",
  "nodes": [
    { "id": "map",    "phase": "Find", "role": "reviewer",
      "model": "gpt-5.5", "effort": "low",
      "instruction": "Scan ONE changed file for correctness bugs",
      "fanout": "1 per changed file" },   // symbolic multiplicity; null = single
    { "id": "reduce", "phase": "Rank", "role": "synthesizer",
      "model": "opus", "effort": "high",
      "instruction": "Dedupe and rank all findings by severity",
      "fanout": null }
  ],
  "edges": [ { "from": "map", "to": "reduce", "kind": "fan-in" } ]
}
```

Example 2 — a **tournament** (different shape, same schema):

```jsonc
{
  "workflow": "design-api",
  "pattern": "tournament",
  "goal": "Produce the best API design from competing attempts",
  "nodes": [
    { "id": "attempt", "phase": "Generate", "role": "designer",
      "model": "opus", "effort": "high",
      "instruction": "Design the API from a distinct angle (given by index)",
      "fanout": "3 attempts" },
    { "id": "judge",   "phase": "Score", "role": "judge",
      "model": "fable", "effort": "high",
      "instruction": "Score every attempt on clarity, safety, ergonomics",
      "fanout": "1 per attempt" },
    { "id": "synth",   "phase": "Synthesize", "role": "synthesizer",
      "model": "opus", "effort": "high",
      "instruction": "Build the final design from the winner + best grafts",
      "fanout": null }
  ],
  "edges": [
    { "from": "attempt", "to": "judge", "kind": "fan-out-then-score" },
    { "from": "judge",   "to": "synth", "kind": "fan-in" }
  ]
}
```

**Schema notes**
- `model`/`effort` are the model's *declared intent* for review. Per the house
  rule, the engine's real source of truth is the role's profile; v1 reconciles by
  having the agent-as-compiler emit a workflow whose roles resolve to those
  models. Native engine-honored per-node model/effort is a fast-follow.
- `fanout` is **symbolic** ("1 per file"), never a fabricated count —
  data-dependent fan-out is unknown pre-run, and reviewing the *pattern* is the
  point.

### The three pieces to build

1. **The skill (the leverage).** Instructs the model: when the user wants to
   design/review a workflow, emit a **blueprint** (name the pattern, one node per
   role/step, per-node model/effort/instruction, symbolic `fanout`, edges) and
   **wait for approval** before authoring + running. On "edit" → revise & re-emit;
   on "approve" → author a workflow faithful to the blueprint and run it. Carries
   the hard part (shape abstraction, pattern naming) so the renderer stays thin.
2. **The thin renderer.** Deterministic **blueprint → graph**. v1 target:
   **blueprint → Mermaid flowchart** (nodes show `role · model · effort`, edges
   annotated with `fanout`). Same input → same picture. A small, testable
   function, not a graph engine.
3. **The propose → approve handshake (the only genuinely new plumbing).** How the
   blueprint reaches the panel and how the run waits for the click. v1: the model
   surfaces the blueprint as a structured block the panel intercepts and renders
   with the three buttons; **approve** signals the session to author + `workflow
   run`; **edit** sends the change back to the model. This is the one part
   needing t3code-side design.

### v1 scope vs. fast-follows

**In v1 (prototype, engine untouched):** skill emits blueprint; thin renderer →
Mermaid graph; approve/reject/edit-with-the-model; agent-as-compiler runs via
existing `workflow run`; review scoped to blueprint-declared workflows.

**Deferred fast-follows (each its own lane):**
- **Native declarative execution** — engine honors per-node model/effort/
  instruction from a plan IR (authority + Boa boundary; line-by-line review;
  digest-bound approve so "what you approved" provably "ran"). v1 fakes it via the
  compiler step.
- **Direct-manipulation editing** (drag nodes / edit fields).
- **Blank-canvas authoring.**
- **Live per-spawn review** for truly dynamic scripts (approve-at-admission +
  richer monitor).

### Honest caveats (acceptable for a prototype, named on purpose)

1. The graph is the model's **declared intent**, not an engine-verified plan; a
   truly dynamic script could diverge. v1 scopes review to blueprint-declared
   workflows.
2. `model`/`effort` are **advisory** in v1 (reconciled via the compiler), not yet
   engine-enforced per node.
3. **No integrity binding** in v1 between the drawn blueprint and the executed
   script beyond the model's faithfulness. Past prototype it gains a digest-bound
   approve (like `plan_digest`/`surface_digest`).

### Open question to resolve before building C2

The **handshake shape**: blueprint rides in-band as a structured chat/tool block
(simplest, recommended for v1) vs a dedicated `workflow propose --json` read + a
fixed approve action (the eventual contract home). Start in-band, migrate to the
contract once v1 proves the experience.

---

## 7. Sequence & the launch-date decision

**Recommended order** (cheapest-first, dependency-safe; one boundary-touching
item in implementation/review at a time):

1. **Lane A** — finish onboarding (smallest, highest first-run payoff).
2. **Lane C1** — workflow observe contract (cheap).
3. **Lane B** — skills/profiles load+activate+create (biggest everyday-value gap).
4. **Lane C2 v1** — reviewable workflows (skill + renderer + handshake); start the
   handshake prototype first to feel the experience.

**Launch-date decision — resolved:** the reviewable-workflow experience ships in
the launch gate as **C2 v1 (engine untouched)**, so the headline ("design the
workflow, see its shape, approve it, run it") is real without gating the release
on native declarative execution. The heavy, security-sensitive engine work is a
**post-launch fast-follow**, designed properly rather than rushed onto the
riskiest surface right before going public.

### Scope guards (what this is NOT)
- Not a second UI — extend t3code + the CLI contract only.
- Not wiring MCP tools into the panel — new capability = new fixed argv actions.
- No inline manifest field-editor for launch (Lane B = load/activate/create).
- No weakening of digest-binding, re-lock/re-render, or the trust gate to shorten
  any lane.

---

## 8. To-do list

Ordered. `[ ]` = not started, `[~]` = in progress, `[x]` = done + verified.
A checked item means implemented and verified, not merely designed.

### Lane A — onboarding finish
- [x] A1 — t3code: detect missing `agentstack` binary → install-guidance card
  *(2026-07-24: detection was already wired; added plain-language headline +
  "Check again" affordance)*
- [x] A2 — t3code: SetupPanel mode/allow picker (secrets `env|keychain|skip` +
  per-import allows); confirm no new CLI surface needed
  *(2026-07-24: secrets picker shipped, `.env` default, plan re-read per choice
  because `plan_digest` binds the destination — no new CLI surface needed for
  secrets. **Per-import allows DO need new CLI surface** — a selection flag on
  `init`/`init --plan` plus digest coverage of the selected subset — deferred
  as its own item.)*

### Lane C1 — workflow observe contract
- [ ] C1.1 — envelope `workflow list`/`workflow runs` JSON in `ui_contract`
- [ ] C1.2 — add `workflow-observe-v1` to the `features` set
- [ ] C1.3 — t3code: negotiate the new feature; monitor consumes the enveloped read

### Lane B — skills & profiles (load + activate + create)
- [x] B1 — CLI: fixed actions add-skill/add-server/create-profile/use-profile
  (enveloped, digest-bound, manifest → re-lock → re-render)
  *(2026-07-24: `crates/cli/src/commands/panel_edit.rs`; apply-setup pattern —
  bare call = preview with `consent_digest`, apply = `--yes --consented`;
  single-path: wraps the same `add::*_json` builders + `use_profile::run` the
  MCP tools use; MCP `add_server` logic moved into `add.rs`, not duplicated)*
- [x] B2 — CLI: `profiles-edit-v1` feature + panel-argv read for the library index
  *(2026-07-24: `library-index` read, additive feature, no schema bump)*
- [x] B3 — extend `t3code_parity.rs` dispatch + witnesses (re-lock/re-render;
  fail-closed on unresolved `${REF}`)
  *(2026-07-24: 5 dispatch arms + 3 witnesses incl. digest stability/binding)*
- [x] B4 — t3code: library browser + "add to toolset" + "new toolset" UI
  *(2026-07-24: gated on `profiles-edit-v1`; preview→digest→apply round-trip;
  unresolved-`${REF}` block renders a what/why/next-step card)*

### Lane C2 v1 — reviewable workflows
- [ ] C2.1 — define the blueprint JSON schema (topology + per-node
  model/effort/instruction + pattern + symbolic fanout)
- [ ] C2.2 — write the skill: model emits the blueprint, waits for approval,
  edit/approve behavior
- [ ] C2.3 — thin renderer: blueprint → Mermaid (deterministic, tested)
- [ ] C2.4 — **prototype the propose→approve handshake first** (in-band block +
  3 actions); resolve the §6 open question against the feel
- [ ] C2.5 — t3code: graph-review panel (render + approve / reject / edit-with-model)
- [ ] C2.6 — agent-as-compiler: on approve, author the workflow from the blueprint
  and run via existing `workflow run`

### Before build
- [ ] Get explicit go for Lanes B and C2 (they touch review boundaries) before
  entering `TODO.md`
- [ ] Slot accepted lanes into `TODO.md`: A → §1.2/§1.3; C1 → §1.3; B → §2.4;
  C2 → new capability lane (needs approval gate)
