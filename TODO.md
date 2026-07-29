# AgentStack execution roadmap

> **Purpose:** the only ordered product-wide work queue
>
> **Strategy:** [`STRATEGY.md`](STRATEGY.md)
>
> **Updated:** 2026-07-29
>
> **Rule:** finish the current stage gate before starting a later product stage

## How to use this file

- Work from top to bottom.
- Keep one item in implementation or review at a time when it touches a
  security boundary.
- A checked item means implemented and verified, not merely designed.
- Security findings can interrupt the product sequence; speculative features
  cannot.
- Closed implementation history belongs in `CHANGELOG.md` or commit history,
  not in this queue.
- Design documents explain decisions. They do not independently authorize
  roadmap work.

## Current objective

Make AgentStack’s everyday value obvious:

> **Import one agent setup, use it across every coding CLI, switch it by task,
> and recover safely when configuration changes.**

The current sequence is:

```text
confirmed fixes
      ↓
first-value journey in t3code + CLI
      ↓
profiles and sessions
      ↓
lifecycle confidence
      ↓
sharing evidence
      ↓
advanced expansion only if earned
```

### Launch status (2026-07-29)

The public-launch path — `install → init → apply → doctor → restore` — is
shipped and public in v0.16.0. What separates here from a validated launch:

- **The §1.6 activation study** (the Stage 1 gate), then fixing the three most
  common blockers it finds. It is the one planned activity that can falsify
  the product thesis rather than confirm it.
- Workflows ship visible and honestly labeled experimental; their promotion
  gate (under "Experimental workflows") is separate and does not block launch.

## Open review findings — what two product reviews left standing

Both 2026-07-27 reviews are closed except the items below. Everything else they
raised is fixed and shipped in v0.16.0; `CHANGELOG.md` is the record of which.
These are kept here, short, because a finding that is neither fixed nor written
down is a finding that gets rediscovered by a stranger.

Ordered by what evidence they are waiting on, not by severity:

- [x] **F01 — one release truth is public.** v0.16.0 is GitHub's latest
  release with provenance-verified archives, checksums, the corrected formula,
  and the public Homebrew tap; installer and `brew install` verified 2026-07-28.
- [ ] **C2 / F04 — the activation study.** Five developers with 2+ supported
  CLIs, observed without command coaching; pass is 4/5 unaided and a median
  under five minutes. Detail in §1.6 and the Stage 1 gate below. Both reviews
  concluded this is now cheaper *after* a public release than before it — the
  gate had been blocking indefinitely, and real installs are where the five
  come from. It is also the only planned activity that could falsify the
  product thesis rather than confirm it.
- [ ] **M1 / F08 — extract the authority data path.** 82% of the workspace's
  Rust lives in `crates/cli`, so `grant.rs` (authority construction) sits in
  the same crate as `lib.rs` (library management) with no compiler-enforced
  boundary. The contract comes first — the existing item under "Engineering
  foundation track" below is the live one. (Sizing note, 2026-07-29:
  `grant.rs` is ~83% tests; the production authority code is roughly 500
  lines, so the move is smaller than the raw line count suggests.)
- [ ] **F09 — one versioned docs system.** Pages now carry a banner naming the
  build they describe, which was the load-bearing half. Full per-release
  versioning, global search, and one content source remain.
- [x] **F15 — browser and accessibility checks at release grade** (2026-07-28:
  phone + desktop widths, landmarks, keyboard skip, themes, reduced motion,
  axe WCAG A/AA; smoke self-test and full local run green).
- [x] **F18 — migration recipes** (2026-07-28: one generated page covering
  Claude + Codex, Cursor + Gemini, dotfiles, teams without shared secrets, and
  complete removal; link/sitemap/a11y checks green).
- [ ] **F19 — a privacy-respecting learning loop.** Local-only by default, with
  an inspectable export of outcomes — never paths, commands, or content.
  Interviews and diary studies before any telemetry.
- [x] **M12 — closed-item narratives belong in `CHANGELOG.md`** (2026-07-29:
  executed — every checked item in this file is now one line plus a date or
  commit ref; the narratives live in `CHANGELOG.md` and commit history. Stale
  "uncommitted" annotations were corrected against git: the scaling lane,
  Lanes C1/C2, and the F13/F14 fixes all shipped in v0.16.0.)
- [x] **F13 — governance trigger met by the planned public launch** (2026-07-28:
  Code of Conduct, succession/funding posture, support routes published and
  linked).

## Stage 0 — close confirmed correctness gaps

Closed 2026-07-23. Detail in `CHANGELOG.md` (v0.16.0) and the commits below.

### Workflow module boundary

- [x] Boa `IdleModuleLoader` landed and independently reviewed; refuses every
  import (`b05fd26`, witness `dynamic_import_of_real_on_disk_module_is_refused`).
- [x] Ambient-capability review of context construction, two independent
  reviewers (2026-07-23): timezone pinned to UTC, `WeakRef`/
  `FinalizationRegistry` poisoned; dynamic-compilation denial and runtime
  limits verified sound.
- [x] §9.3 script-boundary review discharged with zero blocking findings
  (2026-07-23); the `(preview)` label dropped — `workflow` is a visible
  command. Its open follow-ups live under "Experimental workflows" below.

### Consent snapshot and UI authorization

- [x] Immutable `ConsentSnapshot` with focused trust/CLI witnesses (`e1c8000`).
- [x] Independent line-by-line review of the consent/grant path; nine findings
  closed same day; consent digest v3 re-gates every existing entry
  (2026-07-23).
- [x] t3code half of the contract: digest-bound grants, the `agentstack:admin`
  authorization boundary, and version-mismatch fail-closed behavior
  (CLI envelope `717f29d`; t3code `f0196e536`, `d98b5080d`).

### Stage 0 gate

- [x] Closed 2026-07-23: security diffs reviewed line-by-line, focused suites
  green, the t3code trust flow proven end-to-end against the real binary, and
  no write guarantee enforced only by the frontend.

## Stage 1 — first value in under five minutes

### 1.1 Positioning reset

- [x] Complete (2026-07-23): strategy, README, website, and the public docs
  lead with cross-CLI portability; deep security material moved to where it
  becomes relevant; `ENFORCEMENT.md`/`ARCHITECTURE.md` remain the untouched
  authoritative layer.

### 1.2 One recommended onboarding journey

The default journey is:

```text
install → init → review import → apply → doctor
```

t3code presents this as a guided graphical flow; the terminal presents the
same sequence directly. Both must call the same CLI-owned planning, validation,
write, and status paths.

- [x] Complete (2026-07-23, `e1c8000` + same-day batch): init audited from a
  clean machine; `init --plan` is the stable read-only JSON contract with a
  no-write witness; detection evidence, the pre-write import review, lossy
  imports in plain language, visible destinations, and one success summary all
  ship on both surfaces; a failed target no longer hides successful ones; one
  `restore --last` returns the onboarding write set byte-for-byte.

### 1.3 t3code launch experience

- [x] Complete (2026-07-23/24): integration copy replaced with the shipped
  contract; capability negotiation (`717f29d` + t3code `d98b5080d`); setup RPC
  backed by `init --plan` with the four-group setup card; fixed closed actions
  only (consent-bound `setup-apply`, `doctor --json` status, id-addressed
  `restore-write`); server-side workspace identity; one recommended next
  action; no advanced nouns in setup; parity tests prove panel and CLI produce
  byte-identical files (`crates/cli/tests/t3code_parity.rs`); Lane C1
  workflow-observe contract shipped in v0.16.0 (`7b9e101`).

### 1.4 Progressive-disclosure acceptance

- [x] Complete except the user test (2026-07-23 → 2026-07-28): Setup/Toolset/
  Status/Undo are the only beginner concepts; first-run UI speaks outcome
  language; the ordinary import/apply journey is witnessed vocabulary-free
  (`ordinary_journey_vocab`); trust review shows the exact content surface;
  every surfaced denial renders what/boundary/protected/next/details; stronger
  modes sit behind "More protection" with honest cost/coverage labels; one
  canonical ladder (Unify · Switch · Diagnose · Recover · Share · Govern)
  across README, landing, guide, and tutorial; `toolset list` gives the taught
  noun a read; the released v0.16.0 serves the whole t3code panel.
- [x] The recovery pair `restore`/`adopt` joined the default `--help` — Undo
  is a beginner concept, so the way back is findable without `--help --all`
  (`8f96af3`, 2026-07-29).
- [ ] Test the first-run copy with users who have not read the security docs.

### 1.5 First-value proof

- [x] Complete (2026-07-23/28): self-asserting sandboxed demo
  (`examples/first-value-demo/run-demo.sh`); the recording embedded in README,
  landing, and start guide is the current binary's own output as an animated
  SVG; any output-shape drift fails the demo script nonzero, so CI keeps it
  honest.

### 1.6 Activation study

- [ ] Recruit five developers who use at least two supported agent CLIs and did
  not build AgentStack.
- [ ] Observe them without guiding individual commands.
- [ ] Record:
  - install success;
  - time to understand the product;
  - time to first manifest;
  - time to first successful apply;
  - time to clean doctor;
  - confusing terms and abandoned steps.
- [ ] Fix the three most common blockers before adding features.

### Stage 1 gate

- [ ] Four of five users finish without maintainer intervention.
- [ ] Median install-to-clean-doctor time is below five minutes.
- [ ] At least four describe the product as one setup across their coding CLIs.
- [ ] No participant needs Docker, policy authoring, gateway setup, or workflow
  concepts to receive first value.
- [ ] At least four participants understand every block they encounter and can
  choose the safe next action without maintainer explanation.

## Stage 2 — profiles and reversible sessions

### 2.1 Stabilize the profile contract

- [x] Complete (2026-07-23, `e1c8000` + follow-ups): `use --list --json` is the
  machine-readable inventory (name, harness, selection, readiness, trust,
  active state) with one actionable reason per blocked row; the second-toolset
  journey is documented (`docs/howto/name-a-toolset.md`).

### 2.2 Make temporary switching dependable

- [x] Complete (2026-07-23): fail-closed session-start gate (`e1c8000`);
  start/end reports name exactly what they activate and restore; the active
  session is visible in bare `agentstack`; abandoned sessions (12h) are
  defined once and flagged by every surface with the safe recovery command;
  overlapping projects and interrupted processes witnessed
  (`session_overlap.rs`).

### 2.3 Present profiles through user tasks

- [x] Complete (2026-07-23, `docs/howto/name-a-toolset.md`): two worked
  examples; a toolset is "a named subset of the setup you already have — not a
  policy, a permission level, or a workflow role"; sessions are the
  recommended way to switch, static apply the stable/offline path.

### 2.4 t3code toolset picker

- [x] Complete (2026-07-23): read-only toolsets RPC plus the two session verbs,
  name-bound with pre-spawn shape refusals and the CLI's fail-closed gate as
  the enforcement; "Toolsets" / "Use temporarily" language; readiness with one
  reason per blocked row; no create/edit surface in this slice; panel-close
  recovery witnessed. The browser-level walkthrough of a reopened panel
  remains part of the Stage 2 gate's user scenario.

### Stage 2 gate

- [ ] Three users successfully use two different profiles.
- [ ] They start and end sessions without manually editing native files.
- [ ] At least two users return and use profiles in a later session.
- [ ] Interrupted-session recovery works in a user-facing scenario.

## Stage 3 — lifecycle confidence

### 3.1 Connect diagnosis to action

- [x] Complete (2026-07-23): ~110 actionable doctor findings inventoried;
  every finding carries one concrete recommended action (diff / adopt / apply /
  restore / re-lock / re-trust) via `.with_fix()`, feeding doctor's `↳` line
  and its single `start with:` triage; informational restatements dropped;
  blockers stay visually separate from information.

### 3.2 Make writes predictable

- [ ] Standardize dry-run and diff summaries across apply, adopt, init, session,
  and restore.
- [ ] Always distinguish managed, foreign, and hand-edited entries.
- [ ] State whether a write is project-local, user-global, or machine-global.
- [ ] Show the undo path before a material write.
- [ ] Preserve foreign entries unless the user explicitly selects a reviewed
  pruning operation.

### 3.3 Adapter reliability

- [ ] Rank adapters by observed user demand rather than treating all thirteen as
  equally important.
- [ ] Create shared conformance fixtures for the top adapters:
  - import;
  - render;
  - idempotent reapply;
  - hand-edit drift;
  - adopt;
  - restore;
  - secret placeholder behavior.
- [ ] Label lossy adapter fields in import/diff output.
- [ ] Publish the tested behavior matrix.

### 3.4 Recovery scenarios

- [ ] Exercise five end-to-end scenarios:
  - accidental manifest edit;
  - intentional native hand edit;
  - foreign server written by another tool;
  - interrupted temporary session;
  - failed multi-target apply.
- [ ] Ensure each scenario produces a correct diagnosis and safe recovery path.

### Stage 3 gate

- [ ] Five lifecycle scenarios pass without inspecting internal state files.
- [ ] Five users can choose correctly between adopt, apply, and restore from the
  command output alone.
- [ ] Top adapters pass the published lifecycle matrix.

## Stage 4 — sharing and reuse evidence

### 4.1 Team handoff

- [ ] Write a minimal teammate journey:
  clone → inspect → provide local secret values → apply/select profile → doctor.
- [ ] Prove the same manifest and lockfile on two machines.
- [ ] Verify no secret value enters committed files or diagnostic output.
- [ ] Make platform-specific differences visible and actionable.

### 4.2 Library/package reuse

- [ ] Select one real server package and one real skill package used by the
  maintainer.
- [ ] Reuse each across two projects without copying definitions.
- [ ] Measure whether source, lock, trust, and update behavior is understandable.
- [ ] Simplify library terminology or commands based on that exercise.
- [ ] Do not build a public catalog until local reuse succeeds repeatedly.

### 4.3 Team discovery

- [ ] Complete three independent project handoffs.
- [ ] Interview participants about repeated coordination pain.
- [ ] Determine whether the next need is:
  - signed sources;
  - organization policy distribution;
  - hosted profile/package coordination;
  - evidence export;
  - none of the above.

### Stage 4 gate

- [ ] Three project handoffs succeed without credential sharing.
- [ ] One reusable package is used in at least two projects.
- [ ] A repeated team problem—not architectural possibility—selects the next
  expansion.

## Engineering foundation track

This track supports the product stages. It does not authorize unrelated feature
work.

### Extract the authority data path

- [ ] Write a short extraction contract covering:
  `AuthorityGrant → ExecutionPlan → Gateway::try_call → secret resolution /
  upstream transport`.
- [ ] Identify the existing single constructors and dispatch points that must
  remain unique.
- [ ] Move existing code; do not reimplement it.
- [ ] Keep `CompiledRuleset` and `GrantHandoff` as explicit boundary types.
- [ ] Add `#![forbid(unsafe_code)]` to every extracted crate from its first
  commit.
- [ ] Keep the narrowing, trust, pin, secret, and gateway witnesses green.
- [ ] Add a structural check or review rule preventing a second upstream
  transport path.
- [ ] Stop when the CLI is an orchestration caller of the kernel; do not extract
  unrelated library, formatting, or command code merely to improve
  line-count statistics.

### Maintainability

- [ ] Split oversized command modules only when a stable domain seam exists.
- [ ] Keep product terminology consistent across CLI output, docs, JSON, and UI.
- [ ] Generate or verify command reference data where practical to reduce drift.
- [ ] Keep closed work in `CHANGELOG.md` or commit history, not new roadmap or
  memory documents.

### Security and enforcement maintenance

- [ ] Preserve the policy-narrowing property tests.
- [ ] Preserve byte-change trust witnesses.
- [ ] Preserve the single gateway dispatch seam.
- [ ] Keep the enforcement matrix synchronized with shipped behavior.
- [ ] Give the gateway, relay, external harness launch, and workflow interpreter
  comparable adversarial review.
- [ ] Propose new dependencies before adding them.

## Experimental workflows

Workflows remain available for supervised testing but are not part of the
beginner promise.

**Reviewable workflows (Lane C2 v1)** — shipped in v0.16.0 (`7b9e101`,
adversarially reviewed to zero blocking findings): a model proposes a workflow
as an `agentstack-blueprint` JSON blueprint, t3code's chat renderer draws the
shape as a graph, and the user approves / rejects / edits-with-the-model
before it runs. The engine is untouched: the `propose-workflow` skill compiles
on approve and runs through the existing `agentstack workflow run`. Both v1
caveats named at landing are now closed — the approved blueprint is
digest-bound to the executed script (F13, `405ef30`) and declaring is one
recorded, undoable transaction (F14, `dd3e595`). Deferred fast-follows:
native declarative per-node execution, direct-manipulation editing, and the
`workflow propose --json` contract migration. History:
`docs/design/launch-plan.md` (closed record).

Before promoting them out of experimental:

- [x] Complete the module-loader fix and independent script-boundary review
  (§9.3 discharged 2026-07-23, zero blocking findings — recommended
  follow-ups: a cross-model codex pass on quota refill and coverage findings
  A/B/C in the design doc's §9 gate 3).
- [x] Review F13 — the approved blueprint is bound to the executed bytes
  (`405ef30`, v0.16.0).
- [x] Review F14 — compile-on-approve is one recorded, undoable transaction
  through the restore ledger (`dd3e595`, v0.16.0).
- [ ] Review heap-growth and hostile string/regex behavior. (Phase 2b below is
  the piece that actually caps the JS heap the posture label admits is
  uncapped.)
- [ ] Preserve the out-of-thread watchdog and honest posture label.
- [ ] Run at least three recurring tasks on separate occasions.
  (1 of 3 done — 2026-07-23: workflow-acceptance map→reduce→verify, real
  `claude` ×5, PASS, 22.0s, evidence green; see design doc §9 gate 4.
  Occasions 2 and 3 must be separate sittings.)
- [ ] Confirm each task is easier to repeat than the equivalent native/manual
  orchestration.
  (Occasion 1 evidence: 22.0s governed vs 86.4s hand-wired courier path it
  replaces, at the 22.6s ungoverned bookend — governed for ~free.)
- [ ] Confirm roles never widen their selected profile or machine ceiling.
- [ ] Decide whether library distribution is necessary from demonstrated reuse.

### Workflow scaling lane

Design: [`docs/design/workflow-scaling.md`](docs/design/workflow-scaling.md).
MapReduce-informed, but only where the analogy holds: every framework freedom
Hadoop enjoys (retry, reorder, relocate, race) is paid for by task purity, so
the lane buys purity back where a profile can *prove* it, not where an author
claims it. Post-launch: the 2026-07-25 product review puts the everyday loop
ahead of this, and that stands. Phases 0–3 and 5 shipped in v0.16.0
(`ac76fc0`, `49c8f9c`).

- [x] Phase 0 — measurement rig (`examples/workflow-scale/`): deterministic
  replayable mock harness + `analyze.py`. Key finding that set the phase
  order: the batch barrier and the concurrency cap are coupled (efficiency
  0.885 at conc 4 → 0.547 at conc 16).
- [x] Phase 1 — continuous dispatch: persistent worker pool,
  `StepOutcome::Awaiting`; width 100 / conc 16: 15.36s → 12.00s, 0.902 at a
  flat latency distribution — the residual loss is the straggler tail.
  Spawn-evidence-before-launch, lockstep resume replay, park/swap exclusivity,
  and wall-check semantics preserved, with a mutation-verified overlap witness.
- [x] Phase 1b — the serial cliff is visible in `workflow list` (`*` marker +
  `serial_roles` in JSON; contract advertised in `49c8f9c`).
- [x] Phase 2a — schema-validated results: `agent(prompt, {schema})` with a
  bounded JSON-Schema subset and no new dependency. No automatic re-ask — a
  CLI retry would spend an agent slot the engine's ceiling never granted. The
  replay path runs the same transform (witnessed).
- [ ] Phase 2b — content-addressed artifact store
  (`~/.agentstack/artifacts/<sha256>`) plus a resident-result byte cap, past
  which `agent()` returns a frozen opaque `{digest, bytes, preview}` handle.
  This is what actually bounds the JS heap the posture label admits is
  uncapped.
- [x] Phase 3 — `shard()` / `partition()` in the prelude and
  `agentstack workflow explain <name>`: `partition` returns exactly `r`
  buckets with FNV-1a placement so replays reproduce the split; `explain`
  runs the same admission choke point as `run` and reports call *sites*,
  saying so explicitly.
- [~] Phase 4 — purity surface landed and **failing closed**; execution half
  BLOCKED. `[workflows.<n>.scheduling.<role>]` parses `effect_free` / `retry` /
  `speculative`, and validation refuses all three with the prerequisite named.
  The plan's premise was wrong: a `Profile` fences servers/skills/harness
  only, `[policy.filesystem]` is bundle-global and enforced only in sandbox
  mode, and workflow children run at host tier — so nothing today can verify
  effect-freedom. Deriving it would violate rule 8; accepting the author's
  claim would defeat the thesis. Unblocked by either per-profile
  filesystem/egress dimensions, or deriving purity from a sandbox/lockdown
  posture for the role's children.
- [x] Phase 5 — the `Dispatcher` seam with a local-only implementation:
  `TaskDescriptor` carries digests and names, never argv/policy/secrets/paths
  — the absence is the contract and has its own witness; the suite passes
  unchanged and the bench is unmoved.
  - [ ] Still open from Phase 5: cancellation propagation (the watchdog
    orphans in-flight children) and splitting report/list/runs out of
    `workflow.rs` (review F16).
- [ ] Phase 6 — distributed workers. Trigger NOT met as of 2026-07-26, so
  nothing was built: the binding constraint at width 100 is the straggler
  tail, whose fix is Phase 4's backup tasks — blocked on the purity
  prerequisite, not on a shortage of machines.

### t3code MCP harness bridge — research only

t3code already exposes an MCP surface and may be able to launch or supervise
other coding harnesses for a workflow. This could remove duplicated
per-harness process plumbing and make multi-agent workflows visible in the
primary UI. It is not an authorization mechanism and must not become a second
spawn path.

- [x] Inventory the actual t3code MCP tools, authentication, lifecycle,
  cancellation, result, and compatibility behavior
  (`docs/design/t3code-mcp-bridge-research.md`, 2026-07-23). Decision: the
  bridge is NOT buildable on today's surface; the items below stay open
  pending the upstream changes named in that document.
- [ ] Map every proposed MCP operation to the existing workflow child-run
  contract: strict lock, trust, machine policy, frozen `ExecutionPlan`,
  `AuthorityGrant`, scoped MCP configuration, and recorded outcome.
- [ ] Define an optional child-launch backend that accepts only an already
  admitted frozen plan or a narrow launch reference. It must not accept
  arbitrary argv, workspace paths, policy, secrets, or authority from t3code.
- [ ] Add capability negotiation and fail closed when the t3code MCP is absent,
  incompatible, or returns an unrecognized child identity.
- [ ] Prototype one workflow that launches two different harnesses through the
  backend and compare it with direct CLI launch for complexity, portability,
  cancellation, and evidence quality.
- [ ] Add witnesses proving the backend cannot bypass the single child-launch
  dispatch, widen a grant, omit evidence, or leave a child running after
  cancellation.
- [ ] Keep direct CLI child launch as the baseline and fallback. Promote the
  t3code backend only if repeated use shows less integration work without
  weaker authority or evidence.

Deferred until these conditions are met:

- Visual workflow authoring.
- Approval/pause controls.
- Scheduling and durable jobs.
- Cloud workflow execution.
- A generic workflow marketplace.

## Evidence-gated future ideas

The following are deliberately removed from the active roadmap:

- Cloudflare runner.
- Hosted multi-tenant control plane.
- Enterprise assurance program.
- Public registry or marketplace.
- Background jobs and schedules.
- Additional capability categories.
- Separate component repositories.

An idea returns only when:

1. At least three users report the same repeated problem.
2. The smallest useful outcome is defined.
3. Existing features cannot solve it more simply.
4. Success and exit criteria are measurable.
5. It does not displace an unfinished earlier-stage gate.

## Completion definition

The current product strategy is validated when:

- New users reach a clean cross-CLI setup in under five minutes.
- Profiles create repeated use.
- Doctor/diff/adopt/apply/restore provide understandable lifecycle confidence.
- Projects can be handed to another person without sharing secrets.
- Security remains a trusted foundation without being the only visible reason
  to adopt AgentStack.
