# AgentStack execution roadmap

> **Purpose:** the only ordered product-wide work queue
>
> **Strategy:** [`STRATEGY.md`](STRATEGY.md)
>
> **Updated:** 2026-07-23
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

## Stage 0 — close confirmed correctness gaps

These items block broader product work because they violate or weaken an
existing boundary. Finish and review them before enabling new UI writes.

### Workflow module boundary

- [x] Review and land the explicit Boa `IdleModuleLoader` (landed `b05fd26`;
  required witness `dynamic_import_of_real_on_disk_module_is_refused` green;
  independent review 2026-07-23 confirmed the loader refuses every import
  and Boa 0.21.1's default globals expose no filesystem, network,
  environment, or process API).
- [x] Run the focused workflow tests and independently review all context
  construction defaults for other ambient host capabilities (2026-07-23,
  fable + gpt-5.6 Sol independently). Two findings, both fixed with
  witnesses: Boa's default host hook leaked the OS timezone through
  explicit-argument `Date` methods (now pinned to UTC in `Hooks`), and
  `WeakRef` exposed GC-schedule nondeterminism (now poisoned in the prelude
  alongside `FinalizationRegistry`). Dynamic-compilation denial and runtime
  limits verified sound.
- [ ] Keep workflows preview-hidden until the script-boundary review is
  recorded complete (the §9.3 re-run is still pending; its kickoff prompt is
  saved and it gates the preview-label drop).

### Consent snapshot and UI authorization

- [x] Land the immutable `ConsentSnapshot` implementation (`e1c8000`).
  - Manifest, local overlay, and lockfile must be read once.
  - The displayed preview and `surface_digest` must derive from those same
    captured bytes.
  - A grant must record only the digest it verified.
  - An edit before grant must refuse or leave the project in `Changed`, never
    silently bless different bytes.
- [x] Add focused trust and CLI witnesses for absent, wrong, stale, and matching
  consent digests (`e1c8000`).
- [x] Complete the independent line-by-line review of the consent snapshot and
  grant path (2026-07-23, fable + gpt-5.6 Sol independently; nine findings,
  all closed in the same-day hardening commit): interactive grants now record
  the reviewed snapshot's digest instead of a disk re-read; the preview
  refuses to display a library definition that does not match the snapshot's
  lock pin; `apply`'s owned-refresh re-pin digests the bytes it wrote and can
  no longer create or blank a trust entry; the consent digest distinguishes
  absent from empty pinned files (v3 — existing entries re-gate); the
  whole-store load-modify-save is serialized so a grant cannot resurrect a
  concurrent revoke; init's plan digest covers the full import (v2) and the
  consented write consumes the exact verified detection; review/blocker/
  policy lines sanitize hostile text; and the `isatty` consent probe's limits
  are documented honestly in `docs/ENFORCEMENT.md` (a same-user PTY equals
  the same-user store-file boundary, no stronger claim made).
- [x] Complete the t3code half of the contract (t3code `f0196e536`,
  `d98b5080d`): `surface_digest` decoded, carried in the grant request,
  mapped to fixed `--yes --consented-digest` argv, and a grant with an
  absent/malformed digest is refused before anything spawns (stale refusal
  is CLI-enforced).
- [x] Introduce the dedicated `agentstack:admin` authorization boundary
  (t3code `f0196e536`): required for every `agentstackAction` write, granted
  only to administrative sessions, checked server-side against the
  authenticated session's scopes; RPC-level fail-closed witness added in
  `d98b5080d`. Reads stay on `orchestration:read`.
- [x] Verify that older CLI/newer UI and newer CLI/older UI combinations fail
  closed with a useful message (`717f29d` envelope + t3code `d98b5080d`
  negotiation: newer CLI schema → one "update needed" state, actions
  disabled; older CLI without a feature → that action disabled with upgrade
  guidance).

### Stage 0 gate

- [x] Security-sensitive diffs receive line-by-line review (consent path and
  interpreter ambient defaults reviewed independently 2026-07-23; findings
  fixed same day, each with a witness).
- [x] Focused tests pass (trust, workflow, command-module, and integration
  suites green; `cargo fmt --check` and clippy clean).
- [x] The t3code trust flow works end-to-end with a matching digest
  (t3code `d04757e38`: the panel's service drives the real binary through
  preview → grant → drift → stale-digest refusal → re-grant → revoke).
- [x] No frontend condition is the only enforcement of a write guarantee
  (the CLI independently verifies consent digests, plan digests, and
  non-interactive gates; t3code refusals are pre-spawn hygiene on top,
  witnessed by the e2e test refusing at the CLI layer).

Stage 0 is closed except the workflow preview-label item above, which stays
with the pending §9.3 re-run in the experimental workflow lane.

## Stage 1 — first value in under five minutes

### 1.1 Positioning reset

- [x] Replace the security-first strategy with the cross-CLI environment
  manager strategy.
- [x] Make the README lead with “one agent setup across every coding CLI.”
- [x] Align the website hero and contributor orientation with the new product
  definition.
- [x] Review the remaining public documentation for security-first opening copy
  and move deep security material to the point where it becomes relevant
  (2026-07-23, full public-page sweep): the tutorial's first lesson now opens
  with the portability promise (the threat framing moved after it and into the
  Guard/Trust lessons where it belongs); the demos page leads with "the
  promises, proven" and the first-value demo ahead of the malicious-repo one;
  workflows.md defines what a workflow does before its preview/governance
  status; choose.md asks "where do files live" before "how much protection";
  the docs-hub and cookbook openings lead with the outcome instead of
  checksum/sandbox clauses. Task-titled security how-tos (trust-a-repo,
  lock-down-a-run) keep their topic-appropriate openings.
- [x] Keep the enforcement matrix, architecture, and security documentation
  intact as the authoritative deeper layer (verified in the same sweep:
  ENFORCEMENT.md and ARCHITECTURE.md are untouched, the retired-page redirect
  stubs route to them, and every simplified opening links there for the
  precise version).

### 1.2 One recommended onboarding journey

The default journey is:

```text
install → init → review import → apply → doctor
```

t3code presents this as a guided graphical flow; the terminal presents the
same sequence directly. Both must call the same CLI-owned planning, validation,
write, and status paths.

- [x] Audit `agentstack init` from a clean machine/user perspective
  (2026-07-23, sandboxed HOME): flagless non-TTY refuses with named escapes;
  no-CLI machines get the starter manifest; detection distinguishes
  binaries-on-PATH from configs-found; the summary's "From:" names only CLIs
  that actually contributed content.
- [x] Land `init --plan` as the stable, read-only JSON contract for
  detecting CLIs, importable capabilities, secret reference names, origins, and
  proposed destinations without writing or prompting (`e1c8000`).
- [x] Add its witness that no manifest, secret store, native config, or restore
  entry changes during planning (`e1c8000`).
- [x] Ensure the first screen says which CLIs and native configurations were
  found (2026-07-23, both surfaces): scripted init and the wizard open with
  each detected CLI and the exact native config files detection read (or the
  honest "binary on PATH — no config files found"); `init --plan` carries the
  same evidence (`detected[].bin_on_path`, `detected[].configs`) and the
  t3code setup card renders it under "Coding tools found".
- [x] Show imported servers, skills, instructions, and secret reference names
  before writing (2026-07-23): servers are listed by name with what each runs
  or contacts, settings imports name their source CLIs, and lifted secret
  references print with their origins — all before any write; the wizard's
  import confirm moved AFTER that review (`run_for_setup` gates the write).
  Skills and instructions are not part of the init import (nothing to show);
  the plan and terminal state exactly what is imported.
- [x] Explain unsupported or lossy imports in plain language (2026-07-23):
  entries the import cannot map are named with a reason and the assurance
  nothing was deleted, both in `init` output and `init --plan` (`unsupported`);
  the settings import states that unrecognized settings stay in each CLI's own
  file.
- [x] Make the destination scopes and files visible without requiring knowledge
  of adapter internals (2026-07-23, both surfaces): "Files agentstack will
  manage" names the manifest (written by the import) and each CLI's native
  destination with its scope in plain words ("this project" / "machine-wide"),
  labeled as written by the next `apply --write`, not now; `init --plan`'s
  additive `destinations[]` feeds the identical rows in the t3code setup card.
  Witnessed by `init_review.rs` against the real binary.
- [x] End the flow with one concise success summary (2026-07-23): scripted
  `init` closes with manifest path / source CLIs / imported counts / secrets
  still needing values / next commands (`render_import_summary`); the wizard
  close leads with manifest path, CLIs updated, capabilities, and
  still-needed secrets (`render_setup_facts`).
- [x] Ensure a failed target does not hide successful targets or leave ownership
  state ambiguous (2026-07-23): a hard per-target error no longer aborts the
  apply pass — remaining targets render, completed writes stay recorded in
  history and ownership state, the summary names the failures, and the exit
  is nonzero (`apply_partial_failure` witness).
- [x] Confirm `agentstack restore` can undo the onboarding write set
  (2026-07-23): `init_restore_onboarding` witnesses one `restore --last`
  returning manifest, secrets `.env`, and `.gitignore` byte-for-byte.

### 1.3 t3code launch experience

This is the primary graphical path, not an optional dashboard.

- [x] Replace the current t3code integration copy with the product contract
  (`docs/howto/use-with-t3code.md`, `docs/design/ui-control-plane.md`
  updated to the shipped action enum and journey).
- [x] Add capability negotiation (`717f29d` CLI envelope; t3code
  `d98b5080d`): every UI-facing read carries `schema_version` + `features`,
  the panel disables with an upgrade message on mismatch.
- [x] Add a setup RPC backed by `init --plan` (t3code `agentstackSetupPlan`
  → fixed `init --plan` argv; no import logic in TypeScript).
- [x] Present the plan in four user-facing groups (setup card: coding tools
  found / what will be imported / files AgentStack will manage / values
  still needed).
- [x] Add only fixed, closed actions for the first slice: `setup-apply`
  (consent-bound to `plan_digest`), status via `doctor --json`, and
  `restore-write` (id-addressed undo; the ledger is machine-global, so the
  panel undoes the newest entry touching its own project, never `--last`).
- [x] Resolve workspace identity on the t3code server (server-side
  `resolveAgentstackWorkspaceRoot`; the browser sends only project/thread
  ids, never a path or argv).
- [x] Show one recommended next action (`doctor --json` `state` +
  `next_action`; the panel leads with them).
- [x] Keep trust, policy, guard, gateway, and workflow controls out of the
  initial setup screen (the setup card uses plain language only; digests
  live behind a Details disclosure).
- [x] Add parity tests proving the t3code flow and direct CLI flow produce
  the same plan and resulting files
  (`crates/cli/tests/t3code_parity.rs`, `717f29d`: the panel's fixed argv
  and the direct scripted journey yield byte-identical files, the same
  doctor state, and project-isolated undo).
- [x] Remove or rewrite old t3code copy that claimed completeness before the
  consent/admin contract worked (`docs/howto/use-with-t3code.md` now
  describes the enforced contract).

### 1.4 Progressive-disclosure acceptance

- [x] Use only Setup, Toolset, Status, and Undo as beginner navigation concepts
  (2026-07-23, t3code panel restructure): the tab bar is gone — the Overview is
  the single beginner surface (status chip, manifest/checkup/secrets/library
  outcomes, Toolsets card, Undo), and Workflow/Activity/Policy became
  back-navigable advanced views one tap deeper.
- [x] Replace unexplained internal nouns in first-run UI with outcome language
  (2026-07-23): the wizard's delivery-mode picker and orientation drop
  "gateway"/"trust-gated" for outcome phrasing ("nothing on disk — served live
  to your CLIs after review"); the panel's "Doctor" row reads "Checkup", the
  "Gateway" row became "Live serving" inside More protection, and guard/
  sandbox/workflow facts left the first-run overview.
- [x] Verify ordinary local import/apply does not require Docker, policy,
  gateway, confinement, or workflow decisions (2026-07-23:
  `ordinary_journey_vocab` witnesses the scripted init → apply --write →
  doctor journey completing prompt-free with zero advanced vocabulary; the
  unconfigured machine-policy doctor line joined the hidden-by-default
  sections to make that true).
- [x] When unfamiliar repository content triggers review, show the exact
  content surface and why review is required (already shipped in the t3code
  trust review: named servers with their run/contact targets, secret names,
  named skills/workflows/extensions/instructions, and the state sentence
  explaining inertness; `agentstack trust` prints the same surface in the
  terminal).
- [x] For every surfaced denial, render what was blocked, the boundary, what is
  protected, one exact safe next action, and a details link (2026-07-23:
  AgentstackDenialCard now renders all five — blocked command, matching
  rule/source sentence, a derived "protecting" line, the safe next step, and a
  Details disclosure with the rule, dimension, source, and honest coverage
  limits). CLI-side refusals continue to carry what/why/next in their message
  text; a structured JSON denial envelope for UI rendering remains the Slice 3
  design in `docs/design/ui-control-plane.md`.
- [x] Put stronger execution modes behind “More protection” after normal setup,
  with honest cost/coverage labels (2026-07-23: the panel's More protection
  view lists guard, machine policy, live serving, locked runs, and
  sandbox/lockdown — each with live state where readable, what it covers, and
  what it costs, e.g. "host process — not kernel isolation", "needs Docker ·
  slower start").
- [ ] Test the first-run copy with users who have not read the security docs.

### 1.5 First-value proof

- [x] Build one fenced, reproducible demonstration that starts with two real
  native CLI configurations, imports an MCP server, writes one manifest,
  renders two target formats, ends with a clean doctor result, and restores
  the original state byte-for-byte (2026-07-23:
  `examples/first-value-demo/run-demo.sh` — self-asserting, sandboxed,
  asciinema-recordable via `DEMO_PAUSE`).
- [x] Record a short demo focused on portability, not threat prevention
  (2026-07-23: `docs/demos/first-value.gif` + `.cast`, asciinema+agg at
  108×30, DEMO_PAUSE=2.5, recorded against the current binary with all eight
  assertions green in the recorded run).
- [x] Put the same proof sequence in the README, website, and getting-started
  guide (2026-07-23: the five-step import → render → doctor → restore sequence
  with the recording embedded in README "Try it in 60 seconds", the landing
  quick-start card, and start.html Track A, each linking the runnable script).
- [x] Make expected output accurate against the current binary (2026-07-23:
  the embedded output IS the recording of the current binary; `run-demo.sh`
  exits nonzero on any output-shape drift, so CI can keep it honest, and
  start.html's wizard transcript was updated to the Stage 1.4 wording).

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

- [x] Land `use --list --json` as the machine-readable profile inventory
  (`e1c8000`).
- [x] Report the initial machine-readable profile surface (`e1c8000`):
  - name;
  - harness;
  - selected servers and skills;
  - pin/readiness state;
  - project trust state;
  - active state when applicable.
- [x] Give incomplete profiles one actionable explanation rather than several
  low-level lock errors (2026-07-23): the session-start gate — the one place
  that piled a repeated per-item lock line under a "changed since lock was
  written" lead-in false for never-pinned content — now names the unpinned
  items on one line with one fix (lock → review → re-trust → retry); genuine
  drift keeps the accurate story. Gate unchanged, message-shape witness added.
  (`use --list --json` already carries one actionable reason per blocked row.)
- [ ] Document one simple way to create a second profile from an existing setup.

### 2.2 Make temporary switching dependable

- [x] Land the fail-closed session-start readiness gate (`e1c8000`).
- [x] Make `session start` state which profile and native files it activates
  (2026-07-23): the start report names the profile, every native config file
  the session now manages (the exact set `end` restores), the skills it
  materialized where, and the one command that reverts it.
- [x] Make current session/profile visible in the default status surface
  (2026-07-23): bare `agentstack` shows an active session as its own line —
  profile, humanized age, and the end command (the toolsets/doctor JSON
  already carried it via `sessions-v1`).
- [x] Ensure `session end` reports exactly what it restored (2026-07-23):
  the end report lists the files put back to their pre-session bytes (from
  the session's own history entry) and the skills removed; an end with
  nothing to revert says so instead of implying a restore.
- [ ] Detect abandoned sessions and offer the safe recovery command.
- [ ] Test overlapping projects and interrupted processes without silently
  clobbering user files.

### 2.3 Present profiles through user tasks

- [ ] Add two concrete examples:
  - backend development versus incident response;
  - minimal project profile versus broad personal profile.
- [ ] Explain profiles as “named toolsets,” not as policy or workflow roles.
- [ ] Recommend temporary sessions in the beginner path and keep static apply as
  the stable/offline path.

### 2.4 t3code toolset picker

Start only after the CLI JSON contract is reviewed and stable.

- [x] Add read-only profiles RPC using server-resolved workspace identity
  (2026-07-23: `agentstack.toolsets` → fixed `use --list --json` argv;
  `sessions-v1` feature gates the session verbs; body carries per-profile
  `active` and the top-level `session` object).
- [x] Add fixed actions for session start and end (2026-07-23:
  `session-start` name-bound to the toolsets read with a pre-spawn shape
  refusal, `session-end` fixed argv, never `--all`; the CLI's fail-closed
  gate stays the enforcement — witnessed against the real binary in
  `AgentstackCli.e2e.test.ts`, refused until trusted and pinned).
- [x] Label profiles as toolsets in the UI; keep the profile identifier in
  details and machine-readable contracts (panel card says "Toolsets" / "Use
  temporarily" / "Stop using"; the wire contract keeps `profiles`).
- [x] Show readiness and the effective selected surface (per-row server/skill
  counts + harness; a blocked row shows one actionable reason — trust review
  first, else the first blocker's own fix).
- [x] Keep editing/creating profiles out of this slice (read + the two
  session verbs only; no create/edit surface anywhere).
- [x] Demonstrate recovery when the panel closes during an active session
  (2026-07-23: session state is read from the CLI's store on every load —
  `toolset_sessions.rs` witnesses the listing reporting a session whose
  supervisor died, and the panel's "Stop using" maps to the same
  `session end` the e2e proves reverts it). Browser-level walkthrough of the
  reopened panel remains part of the Stage 2 gate's user scenario.

### Stage 2 gate

- [ ] Three users successfully use two different profiles.
- [ ] They start and end sessions without manually editing native files.
- [ ] At least two users return and use profiles in a later session.
- [ ] Interrupted-session recovery works in a user-facing scenario.

## Stage 3 — lifecycle confidence

### 3.1 Connect diagnosis to action

- [ ] Inventory every actionable `doctor` finding.
- [ ] Map each finding to one recommended next action:
  - inspect with `diff`;
  - keep with `adopt --write`;
  - reconcile with `apply --write`;
  - recover with `restore`;
  - re-lock or re-trust when content changed.
- [ ] Remove findings that only restate internal state without helping the user.
- [ ] Keep informational findings visually separate from blockers.

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

Before promoting them:

- [ ] Complete the module-loader fix and independent script-boundary review.
- [ ] Review heap-growth and hostile string/regex behavior.
- [ ] Preserve the out-of-thread watchdog and honest posture label.
- [ ] Run at least three recurring tasks on separate occasions.
- [ ] Confirm each task is easier to repeat than the equivalent native/manual
  orchestration.
- [ ] Confirm roles never widen their selected profile or machine ceiling.
- [ ] Decide whether library distribution is necessary from demonstrated reuse.

### t3code MCP harness bridge — research only

t3code already exposes an MCP surface and may be able to launch or supervise
other coding harnesses for a workflow. This could remove duplicated
per-harness process plumbing and make multi-agent workflows visible in the
primary UI. It is not an authorization mechanism and must not become a second
spawn path.

- [x] Inventory the actual t3code MCP tools, authentication, lifecycle,
  cancellation, result, and compatibility behavior
  (`docs/design/t3code-mcp-bridge-research.md`, 2026-07-23): the MCP
  endpoint is browser-preview only (13 `preview_*` tools, per-thread bearer,
  inward-facing); the `/ws` session protocol launches only pre-configured
  provider instances with no per-call argv, process identity, process-level
  result, or version handshake. Decision: the bridge is NOT buildable on
  today's surface; the remaining items below stay open pending the upstream
  changes named in that document.
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
