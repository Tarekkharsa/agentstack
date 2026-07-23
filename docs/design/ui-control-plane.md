# Design: the UI control plane (AgentStack governance from t3code)

> **Status:** design draft, 2026-07-23. Not yet scheduled against a TODO phase.
> **Scope:** spans two repos — new **CLI primitives** in `agentstack`, and the
> **UI + RPCs** in `t3code` (`~/Documents/GitHub/t3code`, branch
> `agentstack-panel`).
> **Prereqs / related:** [`t3code-panel-design.dc.html`](t3code-panel-design.dc.html)
> (the shipped read-only panel), [`t3code-upstream-pr.md`](t3code-upstream-pr.md)
> (the current PR draft and its read-vs-write decision),
> [`workflows-capability.md`](workflows-capability.md) (the engine this builds on),
> [`locked-run-contract.md`](locked-run-contract.md) (child-run semantics).

## 1. What we're building and why

Today the panel is a **window** onto AgentStack governance (Overview / Workflow /
Activity / Policy tabs, all read-only, plus a small closed set of write actions:
`apply`, `guard install`, and — most recently — `trust-grant` / `trust-revoke`).

The maintainer's goal is to turn it into a **control room** for non-power users:

- **The CLI is for power users.** A normal t3code user should be able to set up
  governance, pick what a session loads, and author + supervise workflows without
  ever opening a terminal.
- Three concrete experiences drive this doc:
  1. **Onboarding a project that has no manifest at all** — a guided `init` in the UI.
  2. **Choosing / changing the "profile"** (the skills + MCP/extensions a session
     loads) when starting work.
  3. **Authoring and supervising workflows** — design the multi-agent architecture
     conversationally, then run it with a controllable "master" (map-reduce style).

This doc exists so a fresh session can implement it without re-deriving the
security reasoning. **The reasoning is the hard part; the wiring is small.**

## 2. The one principle that governs the whole design

Every feature we expose is exactly one of three kinds, and the kind decides where
the enforcement lives:

| Kind | Examples | Rule |
|---|---|---|
| **Read** | run doctor, view a workflow run, list profiles, preview trust surface | Free. Build freely. |
| **Select / act reversibly** | switch to an existing profile, start/abort a run, approve-next mapper | Cheap and safe — **as long as it is safe when the UI is bypassed and the RPC is called directly.** |
| **Commit new surface** | `init` a manifest, define a new profile, save a new workflow | This is a **trust event.** It always converges on the *same* review-and-confirm gate. |

**The resolution to "don't expose trust as a UI feature":** trust is not a feature
you bolt on — it is the **checkpoint that every category-3 flow ends at.** Init
flows into it. Save-profile flows into it. Save-workflow flows into it. You build
the review-and-confirm surface **once** and every write flow walks into it. That is
exactly what content-bound trust is supposed to feel like: the honest last step of
every edit.

### 2.1 Non-negotiable invariants (these gate every PR in this lane)

These restate the project's [`CLAUDE.md`](../../CLAUDE.md) rules for this feature set.
A change that violates one is wrong even if a task description asks for it.

1. **Enforcement lives in the CLI/server, never in the frontend.** The UI may
   *render* a guarantee (e.g. "we showed you the surface before the grant button"),
   but the guarantee itself must hold when a second RPC client, a future refactor,
   or an attacker calls the endpoint directly with no UI in the loop. If the safety
   of an action depends on the UI having shown something first, push that check
   **down** into the CLI before wiring the button. (This is the exact gap flagged in
   the current trust-from-UI commit — see §7.)
2. **Untrusted means inert.** No manifest / untrusted digest ⇒ no skill loads, no
   MCP spawns, no secret resolves. The UI's honest state for an ungoverned project
   is "not governed yet — set it up?", never a silent partial load.
3. **Any pinned byte changes ⇒ re-gate.** Editing a manifest/profile/workflow from
   the UI changes bytes, so it re-gates and must pass the review before it goes live.
4. **The UI can never produce an effective policy more permissive than the machine
   ceiling.** Every child run intersects with the machine policy *per child*. The UI
   shows **effective** (intersected) capability, not requested.
5. **No edit-and-continue on recorded evidence.** The master may inspect / approve /
   abort / rerun. It may **never** inject or edit a step result — that defeats the
   digest-verified journal (rule 4 in CLAUDE.md).
6. **Clients never send filesystem paths.** Every RPC takes `{projectId, threadId?}`;
   a server-side resolver derives the workspace root (existing pattern —
   `resolveAgentstackWorkspaceRoot` in t3code `ws.ts`). The CLI is never pointed at a
   client-supplied path.
7. **CLI output is untrusted input.** args-array spawn (no shell), bounded stdout,
   timeouts, schema-decoded with graceful null on mismatch. Display strings are
   sanitized (`text::sanitize_line`) CLI-side.

## 3. What already exists to build on (do not re-implement)

Grounded in the current tree — most of the spine is shipped:

**AgentStack CLI:**
- `agentstack init` ([`init.rs`](../../crates/cli/src/commands/init.rs)) — "never a
  blank page": detects installed CLIs, imports their MCP servers into one manifest,
  lifts inline secrets into `${REF}`s (destination: gitignored `.env` default /
  keychain / skip). **Every write is captured in the `restore` undo ledger.**
- `agentstack trust <path> --preview` ([`trust.rs`](../../crates/cli/src/commands/trust.rs))
  — emits the runtime consent surface as JSON (state, re_trust, servers with
  run/contact target, secrets, category counts), grants **nothing**. Read-only.
- `agentstack doctor --json` ([`doctor.rs`](../../crates/cli/src/commands/doctor.rs))
  — now carries a machine-readable top-level `trust` field
  (`trusted` / `drifted` / `untrusted` / null).
- `agentstack session start <profile> [--scope]` / `session end [--all]` /
  `session freeze` ([`session.rs`](../../crates/cli/src/commands/session.rs)) —
  **ephemeral, revertible** sessions ("clean-at-rest" mode). If the dashboard dies
  mid-session, `session end` still reverts. `freeze` snapshots a live session into a
  replayable profile.
- `agentstack use <profile> --scope project --write` — **static** activation
  (materialize `.mcp.json` / `.claude/skills/` on disk, gitignored managed block).
- `Profile { servers, skills, harness }`
  ([`model.rs`](../../crates/core/src/manifest/model.rs)) — a profile already selects
  a subset of servers + skills, **and already carries a per-role `harness`** (the CLI
  a workflow role launches). Multi-CLI-per-agent is anticipated in the model.
- Workflow engine: `workflow run|report|list` (`--json` on report/list), the
  digest-pinned child-run drive loop, and **Stage F replay**
  ([`workflow_replay.rs`](../../crates/cli/src/commands/workflow_replay.rs)) —
  re-runs a workflow against its digest-verified journal and refuses on divergence.
- Recorder — per-run `events.jsonl` (`StepSpawned`, step results, `watchdog_kill`,
  `WorkflowResumed`) + global call audit.

**t3code panel (branch `agentstack-panel`):**
- `AgentstackCli` service, args-array spawn, bounded/timed, schema-decoded.
- RPCs: `agentstack.status` / `.activity` / `.workflow` / `.trustPreview`
  (`orchestration:read`), `agentstack.action` (`orchestration:operate`).
- `AgentstackActionKind` closed enum → fixed argv (client never supplies a command line).
- The four-tab panel + guard-denial card + trust review dialog.

## 4. Lane A — onboarding a project with no manifest

**Scenario:** user opens a new thread on a project directory that has no
`.agentstack/agentstack.toml`.

**Honest current behavior:** the project is inert. Panel state = "not governed."

**Flow to build:**
1. Panel detects no manifest (from `doctor --json` with no project ⇒ `trust: null`,
   or a dedicated probe) and shows a **"Set up governance"** call to action.
2. **Init preview (read).** New RPC `agentstack.initPlan` → a new CLI mode
   `agentstack init --plan --json` that runs init's **detection** only and returns,
   as data: which CLIs were found, which MCP servers would be imported, which inline
   secrets would be lifted and their proposed destinations. **Writes nothing.**
3. UI renders the plan with the choices init already owns: *import these servers?*
   *secrets → [.env / keychain / skip]?*
4. **Commit (write, trust event).** New action-enum value `init-apply` → fixed argv
   `init` with the chosen secret-store flag. Init writes the manifest (transactional,
   `restore`-undoable).
5. **The flow does not end at "manifest written."** The freshly-written manifest is
   **untrusted**, so the last screen is the existing **trust review dialog**
   (`trust --preview` → confirm). Init-in-UI *terminates in the consent gate.*

**CLI work:** add `init --plan --json` (detection-only, structured output). Reuse the
existing discovery/lift code paths; do not fork them.
**t3code work:** `agentstack.initPlan` (read RPC), `init-apply` action value, the
setup wizard component, then hand off to the existing trust dialog.

## 5. Lane B — choosing / changing the session profile

**Scenario:** user wants a different set of skills + MCP/extensions loaded for this
session.

**The critical distinction (build the UI around it):**
- **Selecting** an existing, already-trusted profile ⇒ cheap, safe, ephemeral. This
  is the frictionless everyday action. Build it first.
- **Defining** a new profile (a skill+server combination never pinned) ⇒ a manifest
  edit ⇒ a trust event ⇒ routes through the review.

**Flow to build:**
1. **List (read).** New RPC `agentstack.profiles` → `agentstack use --list --json`
   (or a small dedicated `profile list --json`): declared profiles + their
   skills/servers/harness + a per-profile "all skills pinned & trusted?" flag.
2. **Select (act, reversible).** Thread-start profile picker → new action value
   `session-start` → `session start <profile> --scope <default>`. Ephemeral;
   `session end` reverts. Pairs with a `session-end` action.
   - **Fail-closed rule:** if the chosen profile references skills/servers whose
     bytes are not pinned/trusted, `session start` must refuse and route to the
     review — never silently load unpinned content (invariant 2).
3. **Define (write, trust event).** "New profile" editor (pick skills + servers +
   harness) → writes `[profiles.<name>]` to the manifest → re-lock → trust review.

**CLI work:** structured `--json` profile listing incl. a pinned/trusted flag per
profile; confirm `session start` fails closed on unpinned surface (add a test if the
guarantee isn't already enforced).
**t3code work:** `agentstack.profiles` read RPC; `session-start` / `session-end`
action values; the picker (cheap path) and the profile editor (trust path).

## 6. Lane C — workflow authoring + the controllable master

This is the largest lane. Two halves: **authoring** (design-time) and **the master
console** (run-time).

### 6.1 Authoring (design-time, pre-trust — cheap and conversational)

The user describes what they want; the model proposes a workflow architecture; they
iterate on the DAG, swap an agent's CLI/model, add skills/instructions per agent.
**None of this is committed, so no gate is needed** — it's design work.

- Represent the in-progress design as an **architecture object** the UI renders
  (agents/roles, their CLI (`harness`), model, skills, instructions, and the
  map/reduce edges). This is a conversational artifact, not yet manifest content.
- **Commit = trust event.** "Save workflow" produces the workflow script + declared
  `[workflows.<name>]` roles pinned into the manifest+lock. The engine already
  governs this surface: **workflow drift and role-widening block trust until
  re-locked.** So each agent's CLI/model/skills/instructions is exactly the pinned
  surface the review dialog shows — the human consents to that shape.
- **Per-agent capability is intersected per child (invariant 4).** The authoring UI
  must show each agent's **effective** capability (after machine-ceiling
  intersection), not what its role *requested*, or users will be baffled when a
  mapper can't reach something its role "declared."

**Open question (§9):** is the authored script *generated* (model writes JS the Boa
engine runs) or *assembled* from a constrained template? Generated scripts are
hostile input the engine already sandboxes, but generation-in-UI widens what a
"save" can pin. Lean template-first; revisit.

### 6.2 The master console (run-time)

The maintainer's Hadoop mental model maps almost one-to-one onto the shipped engine:

| Hadoop | AgentStack workflow engine (already built) |
|---|---|
| Master tracks per-split metadata | Recorder journals every `StepSpawned` + result per child run |
| Mapper / reducer processes | Governed child runs — each a locked run with a digest-pinned request |
| Master reruns a failed/killed task | **Stage F replay** — reruns against digest-verified history, refuses on divergence |
| Master kills a hung task | `watchdog_kill` |
| Rebuild lost split data | Child-run stdout artifact verified against recorded `sha256` |

**So "make the master controllable" = expose existing engine state + a safe verb set:**

- **Inspect** (read): live monitor — stages, per-agent state/role/tool-counts, pinned
  digest, done/running. Most of this exists in the panel's Workflow tab; extend it
  with a replay-style step timeline over `workflow report --json`.
- **Approve-next** (act): breakpoint-before-spawn — pause the drive loop and let a
  human approve/deny the next mapper spawn. This is human-in-the-loop **admission**,
  on-brand for a governance tool. Approve/deny **only**.
- **Abort** (act): stop a run (maps to `watchdog_kill` / existing kill path).
- **Rerun-from-step** (act): re-run a failed/killed child against the verified
  journal (Stage F replay).

**Forbidden (invariant 5):** edit-a-result-and-continue. Injecting a fabricated
mapper output defeats the digest journal. A master that can fabricate a split's data
is a corruption vector, not a master. "Pause, edit the request, continue" is a
request the engine never admitted — deny by construction.

**CLI work:** a run-control surface for approve-next (requires an engine pause point
before spawn — design against the drive loop, likely the biggest single piece);
structured live run state for the monitor; confirm rerun rides Stage F unchanged.
**t3code work:** the master console UI; `workflow.*` action values (approve / deny /
abort / rerun-from-step) as fixed-argv, each `orchestration:operate` or the new admin
scope (§7).

## 7. Cross-cutting: the scope + consent-binding decision (do first)

Two open items from the current trust-from-UI work block this whole lane and should
be settled before more write actions land:

1. **Dedicated `agentstack:admin` scope.** Trust grant/revoke, `apply`, `guard
   install`, and the workflow control verbs all touch the **security control plane**.
   They currently reuse `orchestration:operate` — the same tier as `vcs.pull`. Adding
   trust and workflow-master control to that tier makes a dedicated
   `agentstack:admin` scope (in t3code's `AuthAdministrativeScopes`) overdue, not
   optional. **Decide before adding more action values.**
2. **Bind consent to what was shown.** Today "the user saw the surface before
   granting" lives only in a React `&&`; the server will run `trust --yes` on request
   regardless (invariant 1 gap). Fix: `trust --preview` emits the previewed **surface
   digest**, and `trust --yes` requires a matching `--consented-digest` or refuses.
   Then "a human reviewed this exact surface" is CLI-enforced, not UI-rendered. Apply
   the same pattern to any future "review then commit" action (init, save-profile,
   save-workflow).

Until these land, keep the read-only slices shippable independently (they already are).

## 8. Suggested sequencing

Ordered by value-to-risk; each step is independently landable.

1. **Settle §7** (admin scope + consent-digest binding). Unblocks everything else and
   closes the current UI-enforcement gap.
2. **Lane B select-only** — profile picker over `agentstack.profiles` +
   `session-start`/`session-end`. Highest value, lowest risk (ephemeral, reversible,
   selecting already-trusted surface). **Ship first.**
3. **Lane A** — `init --plan --json` + setup wizard → hand off to the trust dialog.
4. **Lane C inspect** — the master monitor / replay timeline (read-only) over
   existing `workflow report --json`. No new write surface.
5. **Lane C control** — approve-next / abort / rerun. Requires the engine pause
   point; supervised, line-by-line-reviewed work.
6. **Lane B / Lane C authoring** — profile editor and workflow authoring (the
   category-3 write flows), each terminating in the review gate.

**Note on t3code:** the `agentstack-panel` branch cannot merge upstream yet (t3code
isn't accepting contributions) and is already 6 commits deep on a fast-moving repo.
Keep the **read-only slices** (init plan, profile list, master monitor) splittable
from the **write slices**, so the low-friction parts can land alone when upstream
opens.

## 9. Open questions to resolve during implementation

- **Admin scope shape** — new `agentstack:admin`, or split read/operate/admin three
  ways? (§7.1)
- **Consent-digest mechanics** — what exactly is hashed for the previewed surface,
  and how does `--yes` verify it without re-deriving a second source of truth? (§7.2)
- **Workflow authoring: generated vs templated script.** (§6.1) Lean templated.
- **Where does the engine pause for approve-next?** A pre-spawn admission hook in the
  drive loop — design against Stage F so a paused run is still resumable/replayable.
- **Static vs clean-at-rest for UI profile changes.** Session-based (ephemeral,
  `session start/end`) is the safer default for the UI; `use --write` (static
  materialization) is the power-user path. Confirm the UI only ever drives the
  ephemeral path.
- **Multi-CLI child runs and the machine ceiling.** Confirm the per-child
  intersection already applies to workflow child runs of differing harnesses, and
  that the monitor surfaces the effective (not requested) capability.

## 10. Verification expectations

Per CLAUDE.md — one focused witness per new security-relevant behavior, not
exhaustive suites:

- `init --plan --json` writes nothing (assert manifest absent after).
- `session start` on an unpinned-surface profile refuses (fail-closed).
- consent-digest: `trust --yes` refuses a mismatched/absent `--consented-digest`.
- master control: approve-next denial spawns nothing; rerun-from-step still refuses
  on journal divergence (Stage F invariant intact).
- effective-capability: a child whose role requests more than the ceiling shows the
  intersected capability, and cannot exceed it at run time.
- t3code side: each new action value maps to fixed argv; each new read RPC degrades
  gracefully when the CLI is absent/older (existing `NotFound` pattern).
