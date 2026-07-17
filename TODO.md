# AgentStack execution checklist

> **Status:** active work queue<br/>
> **Updated:** 2026-07-16<br/>
> **Current phase:** Phase 0A **minimum version** (maintainer scope decision
> 2026-07-16; Phase 0B and everything after the minimum-version cut are
> deferred)<br/>
> **Strategy source:** [`STRATEGY.md`](STRATEGY.md)

This is the only ordered day-to-day plan. Start with the first unchecked item
in the current phase. Do not begin a later phase until the current phase's exit
gate is satisfied. `STRATEGY.md` explains why and defines the gates;
`docs/ARCHITECTURE.md` and `docs/ENFORCEMENT.md` define the technical boundaries.

**Minimum-version decision (2026-07-16):** the maintainer is scoping the
project to a minimum version: solve the configuration problem (shipped) and
finish the trust machinery already implemented, then stop. Success criterion
for the cut is "trustworthy for the maintainer's own daily repositories" —
external activation metrics and distribution are deferred with Phase 0B, not
abandoned. Deferred items remain listed so resuming one is a deliberate scope
decision, never an accident.

## How to work from this file

1. Select the first unchecked item in the Phase 0A **minimum version** cut;
   deferred sections are not work sources until the cut ships.
2. Read the linked strategy section and relevant technical contract before coding.
3. For trust, policy, secret, digest, or enforcement semantics, work in a short
   supervised session and require line-by-line review.
4. Add or update an executable witness for every changed security claim.
5. Mark the item complete only after implementation, tests, and documentation agree.
6. Record historical detail in git history or `docs/HISTORY.md`, not in another plan.

## Completed foundation

- [x] Split the implementation into the nine-crate workspace.
- [x] Add content-bound trust, machine-first policy, and lock verification.
- [x] Fail closed when machine policy is corrupt and no valid snapshot exists.
- [x] Add Docker sandbox, lockdown networking, egress enforcement, and recording.
- [x] Build official release binaries with the sandbox backend included.
- [x] Make the gateway the sole MCP authority for declared endpoints under lockdown.
- [x] Add the experimental bounded TypeScript `tools_execute` primitive.
- [x] Publish the current security review and enforcement matrix.

## Phase 0A — prove the canonical protected run

**Details:** [strategy phase 0A](STRATEGY.md#phase-0a--prove-the-canonical-no-docker-activation-path)

**Technical boundaries:** [architecture](docs/ARCHITECTURE.md) · [enforcement matrix](docs/ENFORCEMENT.md)

### Contract

All contract items are satisfied by the approved
[`docs/design/locked-run-contract.md`](docs/design/locked-run-contract.md)
(revision 4, approved 2026-07-15).

- [x] Write the reviewed behavioral contract for
  `agentstack run <harness> --locked` using the current working directory as
  the project.
- [x] Define how `--locked` composes with `--profile`, `--plan`, the required
  harness positional, and trailing harness arguments. (Contract §2.)
- [x] Define the exact no-Docker guarantee: explicit trust, locked-input and
  drift checks, machine-policy ceiling, cooperative host guard, evidence, and
  honest non-isolation limits. (Contract §3, §3.1.)
- [x] Define the maximum-assurance guarantee separately for `--sandbox` and
  `--lockdown`. (Contract §5.)
- [x] Freeze the minimum backend-neutral fields needed later by Workspace
  Grants and hosted adapters without implementing those later phases now.
  (Contract §6: `AuthorityGrant` / `RunEnvelope`.)
- [x] D2: specify how the existing `ExecutionPlan` / `Gateway::from_frozen`
  seam extends across native render and profile leases without a parallel
  authority abstraction. (Contract §7; `from_plan` never existed in code —
  contract §0.)
- [x] D3: decide which stdio scripts and local executables are declared
  integrity inputs for `--locked`; if the first release excludes them, write
  the limitation into the contract and trust preview. (Contract §8: Option A
  with declared integrity roots — implementation pending below.)

### Minimum version — the current cut (2026-07-16)

One keystone increment plus three small closures, plus the marketing surface
(vision decision, later on 2026-07-16: a visitor must see the value
immediately — demos and docs quality are in the cut; outreach stays deferred).
The keystone is the last engineering prerequisite for everything staged on
`feat/d3-local-executable-integrity` (D3 pins, strict verifier, sealed
`AuthorityGrant`, KAT-frozen digest) to become an enforced claim instead of
unwired machinery.

- [ ] Implement the canonical `run <harness> --locked` flow as **one
  supervised increment** (contract §3 sequence): recorder-open
  (`AttemptStarted`) → enforced trust → strict lock verification including D3
  executables (`ensure_locked_inputs`) → policy admission → freeze
  `AuthorityGrant` (`GrantFrozen`) → fallible gateway (zero-server-valid) →
  launch-scoped MCP config with cooperative host guards → recorded outcome.
  Lands `RunEnvelope` (contract §6.2) and the material checked-append recorder
  events (contract §9); consumes the staged `grant.rs`/`verify.rs` surface.
  - **Landed (needs line-by-line review):** everything through GrantFrozen +
    launch + recorded outcome, the checked-append recorder events, `--plan`
    (aggregates all blockers, mutates nothing, digest equals the live run's
    once the commitment key exists; a never-provisioned key is informational
    — "will be created on first live run" — while a present-but-broken key
    still blocks), and loud named-limitation errors for `--locked --profile`
    / `--sandbox`. Verified live: drift refusal, trust re-gate, admission
    (unclassifiable host), clean run with real harness.
  - **Also landed (2026-07-16, rulings: config-scope now / honest global
    label):** launch-scoped PROJECT MCP config (gateway-only during the run,
    parked original restored byte-identical, overlapping runs refuse instead
    of stacking parks, crash leaves the more restrictive state) and
    `RunEnvelope` as the sealed §6.2 evidence identity.
  - **Also landed (2026-07-16, pre-dogfooding round):** five asserted
    use-case example projects (`examples/projects/` + `FINDINGS.md` — the
    skill-indexing report, the CLI-differences matrix, the locked-run device
    test), and the twelve issues that round filed, all fixed the same day
    (#11–#22): guard honors the project `[policy.filesystem]` deny from the
    preferred `.agentstack/` layout; `run` (plain and `--locked`) spawns the
    harness at the project root; `--plan`/live commitment-key parity;
    `report` renders the locked lifecycle and carries the posture slug in
    `--json`; silent drops to instruction-/skills-less adapters warn on every
    surface and instruction `targets` validate like servers'; `agentstack
    search` covers the central library (name + frontmatter description);
    `agentstack_list_loadable` takes a `query`; `lib list` shows
    descriptions.
  - **Also landed (2026-07-17, needs line-by-line review — the three
    remainders):** (a) the run-grant artifact handoff: `live()` writes a
    reviewed `GrantHandoff` projection (ruleset + `${REF}`-only frozen
    servers + project/consent identity; never argv or secret values) into
    the run's private dir, and the launch-scoped bridge entry becomes
    `agentstack mcp --grant <path>` — the bridge consumes it verbatim via
    `Gateway::from_frozen`, fail-closed on schema/ruleset-version skew,
    wrong project, consent staleness (a post-freeze manifest edit refuses),
    and lost trust, never falling back to disk re-derivation; lease
    transitions under a frozen grant are refused honestly (leases-consume-
    the-grant stays with the deferred D2 unification). (b) `--locked
    --profile` as a fence: one-time resolution, gates, grant, artifact, and
    bridge all see the profile's subset (`ProfileEffect::Fenced`, additive
    digest tag, KAT unchanged); no native session state is applied — skills
    render under a locked profile stays with the D2 render unification.
    (c) the user/global-scope warning is now content-derived: it names the
    harness's actual ambient global MCP entries (including Claude Code's
    nested `projects[<dir>].mcpServers`) or reports the scope clean.
    Witnesses: the three `handoff_*` tests in `grant.rs`; verified live
    end-to-end including the staleness refusal.
  - **Remaining (honest limits, not blockers):** actual NEUTRALIZATION of
    ambient global-scope entries on the host tier stays out deliberately —
    the global config is one shared file harness apps rewrite mid-run, so
    park/swap races clobber user state; the sound routes are `--lockdown`
    (kernel fence, shipped) or per-CLI isolation flags in adapter
    descriptors (evidence-driven, not yet designed).
  - The wiring must assert a `GrantedServer`'s definition digest was honestly
    derived from its stored `Server` bytes (carried 3b-ii review note).
    (Done: `GrantedServer::from_resolved` is the only wiring constructor.)
- [x] Call `Lock::retain_executables` so a removed server or integrity root
  prunes its `[[executable]]` pins (stale-pin gap; mirror
  `retain_instruction_names`). (In the strict manifest-global
  `record_executable_pins` — the profile-scoped `record_lock` first-pin path
  sees only a subset of servers and must never prune; pruning also skips when
  any server ref fails to resolve, so an incomplete picture can't drop live
  pins.)
- [x] Witness: a one-byte D3 executable edit fails locked verification before
  launch and re-gates review; intentionally unpinned code is labelled
  honestly. (`one_byte_executable_edit_refuses_before_launch_and_is_recorded`
  + the digest/verifier/lock-cmd layer witnesses; unbound surface labelled in
  the trust preview and locked posture output.)
- [x] README restructure: pain-led hero, 60-second start, three proof blocks
  (guard, trust gate, one-manifest-everywhere), experimental features moved
  out of the beginner path. Includes the honesty pass — every claim matches
  the shipped tier; `--locked` is pre-launch gating, not isolation; D2
  standalone-command unification is a labelled known limit. (Commit bf879ea;
  new guard section with embedded demo gif.)
- [x] Docs site reorganization: index leads with a simple→advanced "See it
  work" ladder (manifest → guard → trust gate → lockdown); pages consolidated
  and status-labelled; stale claims fixed against the code (commit bf879ea).
  Design pass on top: one-row sticky header across all six pages with
  responsive link-hiding, enforcement-matrix cells matched to ENFORCEMENT.md,
  hero proof visible above the fold, tightened section rhythm.
- [x] Three recorded demo clips (asciinema + agg, 100x30, DEMO_PAUSE=2.5):
  guard blocks destructive commands (28s); trust gate (39s); one manifest →
  3 CLIs (33s). docs/demos/*.gif + .cast, guard gif embedded in README.
  (Commit bf879ea. Still deferred: the locked-run demo trio recordings.)

Landed toward this cut:

- [x] D3 end to end short of the locked flow: symlink-rejecting root digests
  (core); auto-detected command/args pins + declared roots recorded by
  `agentstack lock`/`use --write`; strict verifier dimension; trust review
  blocks unpinned/drifted executables; doctor errors on drift/underivable and
  warns on unpinned.
- [x] Canonical V1 `AuthorityGrant` digest, KAT-frozen, keyed argv commitment
  with no unkeyed fallback (contract §4, §6.1).
- [x] Regression witness that authoritative trust/lock verification hashes
  current bytes and never uses the stat-fingerprint digest cache.
  (Landed with the skill-cache bypass fix; contract §3 step 4, ruling 3.)

### Deferred beyond the minimum version

Explicitly deferred, not silently dropped. Each keeps its contract reference;
picking one up again is a deliberate scope decision.

- [ ] D2 standalone-command unification: plain `apply` / `session` / MCP lease
  invoked outside a locked run keep today's behavior. (Contract §7 requires
  this before the full Phase 0A exit gate — that gate is deferred with it.
  The README honesty pass above labels the limit.)
- [ ] Extend the D2 frozen grant across render, gateway, and profile leases,
  and prove every delivery path consumes the same grant and cannot widen
  authority.
- [ ] Remaining recorder events: trust-store mutations and per-run token/cost
  evidence. (The locked flow records its material events; these two dimensions
  stay honestly `unavailable`, which the contract permits.)
- [ ] `agentstack_lease_freeze` → `agentstack_lease_capture` naming decision
  (only matters before external compatibility, which is deferred).
- [ ] Remaining demos beyond the three in-cut clips: the locked-run trio
  (safe repo, policy violation, drift — their substance ships with the
  wiring; recording waits for it) and the Docker maximum-assurance recording.
- [ ] Documentation tooling (the automated kind): claim-consistency tests for
  the enforcement matrix and event list, strategy-page sync, claim tests over
  CLI examples. (The status-label *convention* moved into the in-cut docs
  reorganization; only its test enforcement stays deferred.)
- [ ] Activation measurement: five unassisted strangers, sub-15-minute median
  — deferred with Phase 0B distribution.

### Phase 0A exit gate (deferred with distribution)

Unchanged as the bar for calling Phase 0A *complete*; the minimum version
deliberately ships without it.

- [ ] All three no-Docker demos and the separate Docker demo pass.
- [ ] Five unassisted users complete the flow.
- [ ] Median activation is below 15 minutes.
- [ ] Documentation and enforcement claims agree.

## Phase 0B — validate the problem and distribution

**Deferred (maintainer decision 2026-07-16).** The minimum version ships
without outreach or validation work; this lane resumes when there is time or
a reason to seek external users. Two of this lane's assets — the demo
recordings and the README/homepage rewrite — moved into the minimum-version
cut later the same day (users must see the value immediately); interviews,
publishing, and outreach stay here. The items stay listed so resuming is
deliberate.

**Details:** [strategy phase 0B](STRATEGY.md#phase-0b--validate-the-problem-and-build-distribution)

- [ ] Conduct 20 problem interviews with multi-agent developers, platform
  teams, repository maintainers, or security owners.
- [ ] Record which folders real tasks should read, edit, or never see.
- [ ] Record which agent tasks repeat often enough to become saved workflows.
- [ ] Test willingness to run remotely when the sandbox receives no long-lived
  repository or model credential and returns a patch plus receipt.
- [ ] Reach five interviews that end with “can I try it?”
- [ ] Publish the clone-as-consent technical article.
- [ ] Publish the malicious-repository demo and short recording.
- [ ] Publish the compromised-MCP-server demo and short recording.
- [ ] Publish the changed-pinned-byte demo and short recording.
- [ ] Document the existing GitHub Action with a copyable workflow and visible failures.
- [ ] Rewrite the homepage and README lead around repository consent.
- [ ] Start targeted outreach before a broad launch.
- [ ] Publish a Show HN only after the quickstart works unassisted.

### Phase 0B exit gate

- [ ] Twenty interviews completed.
- [ ] Five qualified people ask to try AgentStack.
- [ ] At least one repeatable acquisition channel produces unassisted activation.

## Phase 1 — productize the wedge and exact Workspace Grants

**Start only after both Phase 0 gates.**

**Details:** [strategy phase 1](STRATEGY.md#phase-1--productize-the-wedge)

- [ ] Make the canonical protected run predictable, documented, and fast.
- [ ] Implement exact directory-root Workspace Grants in the maximum-assurance
  local path, beginning with safely mountable `path/**` roots.
- [ ] Keep the repository root read-only and add nested read-write mounts only
  for approved roots.
- [ ] Reject path traversal, symlink escapes, missing roots, ambiguous write
  globs, and overlaps with deny rules.
- [ ] Implement honest fine-grained read isolation with an empty/sparse
  workspace or deny-mask mounts before claiming scoped reads.
- [ ] Give untrusted bundles no writable submounts.
- [ ] Show the effective read/write/deny grant in `--plan`, trust review, and
  the run report.
- [ ] Bind grants to profiles and future roles such as researcher,
  implementer, reviewer, and deployer.
- [ ] Freeze and version the backend-neutral execution plan: project/workflow
  digests, folders, tools, secrets, egress, commands, approvals, budgets, and
  audit identity.
- [ ] Add conformance tests proving a backend can only preserve or narrow the plan.
- [ ] Resolve D5 after compatibility evidence: decide whether ordinary
  `--sandbox` becomes topologically confined by default and give the weaker
  bridge/proxy-only mode an explicitly weaker name if retained.
- [ ] Add a report-only sequence-anomaly heuristic after recorder completion:
  flag `secret_access` followed shortly by egress inconsistent with that
  secret's constrained server. Never block; label it metadata correlation, not
  DLP or payload inspection.
- [ ] Improve provenance, signature, migration, policy-example, export, install,
  upgrade, and cross-platform UX.

### Phase 1 exit gate

- [ ] Ten independent repositories activated.
- [ ] Five weekly active external users for four consecutive weeks.
- [ ] Three repositories use the GitHub Action.
- [ ] One external team relies on AgentStack for real work.
- [ ] At least one external run proves writes outside its directory grant fail.
- [ ] The frozen plan passes local backend conformance tests.

## Cross-cutting experimental executor stabilization

**Does not displace the active phase. Required before `tools_execute` can leave
experimental status.**

**Security checklist:** [threat model](docs/design/tools-execute-threat-model.md) ·
[enforcement status](docs/ENFORCEMENT.md#experimental-tools_execute)

- [ ] Build a current invariant-to-witness index that links every executor
  security claim to an implemented test and marks unproven cases explicitly.
- [ ] Commission an independent source-to-sink security review.
- [ ] Fuzz relay framing plus request, result, and limit normalization.
- [ ] Soak repeated executions, cancellation, client disconnects, upstream
  hangs, output limits, relay failure, and concurrent clients; prove no
  container, network, process, or thread leaks remain.
- [ ] Cover relay-token reuse, fork/child teardown, direct DNS/UDP/proxy bypass,
  fake-secret and protocol-shaped log data, and post-plan TOCTOU mutation.
- [ ] Publish supported-architecture runtime provenance, executor SBOM,
  independent image scan, digest-update policy, and attestation.
- [ ] Revisit mutating-tool approval only through explicit tool metadata; never
  infer side effects from a tool name.
- [ ] Leave experimental status only after there are no open critical/high
  findings, cleanup is demonstrated, supply-chain evidence is published, and
  `ENFORCEMENT.md` matches every fail-closed behavior and residual limit.

## Native extensions capability lane (added 2026-07-16, post-cut)

**Maintainer scope addition (2026-07-16): govern native harness extensions
(pi extensions, OpenCode plugins) as a first-class capability kind.** Queued
behind the minimum-version keystone — it does not displace the active Phase 0A
item; starting it earlier is a deliberate scope decision. Extensions are
pinned executable content: the strictest kind agentstack manages, and honest
about being provenance-only at runtime (the code runs inside the harness
process, outside the policy ceiling).

**Details:** [`docs/design/extensions-capability.md`](docs/design/extensions-capability.md) ·
ledger entry D6 in [`STRATEGY.md`](STRATEGY.md#security-decision-ledger)

- [x] E0: review and approve the design doc (settle `target` singular,
  copy-render, strict root digest, guard-name reservation, pi + OpenCode
  first, the three open questions).
- [x] E1 (supervised): `[extensions.*]` manifest kind, `[[extension]]` lock
  pinning via the strict `integrity_root_digest`, retain/prune rules,
  distinct trust-preview labelling. Witness: a one-byte extension source edit
  fails locked verification and re-gates review.
  - **Landed + reviewed, commit 3b293c1:** the manifest kind
    (path sources only; git rejected at validation until E3), strict pinning
    + pruning in `agentstack lock`, trust preview blocks unpinned/drifted/
    retargeted extensions, `run --locked` verifies them via
    `ensure_locked_inputs`, and the pin records its `target` so retargeting
    re-gates like drift. Witnesses:
    `one_byte_extension_edit_refuses_locked_and_relock_regates`,
    `extension_verdicts_fail_closed_and_locked_gate_names_them`, plus the
    validation and lock round-trip tests.
- [x] E2 (supervised): render for pi + OpenCode — `ExtensionsSpec` gains a
  write path, ownership ledger, prune path, rendered-copy verification in the
  locked flow, `--plan`/report/posture surfaces. Witnesses: an untrusted
  bundle renders no extension bytes; pruning never touches unmanaged files or
  `agentstack-guard.*`.
  - **Landed + reviewed, commit 3b293c1:** copy-render (never
    symlink) via the strict walk exposed as `integrity_root_files`;
    per-directory ownership ledger keyed by project (multi-project-safe
    global dirs); prune limited to this project's ledger artifacts with the
    guard deny-list enforced at render AND prune; a `rendered-verify` gate in
    `run --locked` (+ `--plan`) that refuses a tampered rendered copy against
    the lock pin. Adversarial review found and fixed two path-traversal
    vulns (forged ledger keys; extension names as paths) — both now have
    witnesses, plus name validation (`InvalidExtensionName`). All four
    E2 witnesses green.
- [x] E3: library `kind: extension` (resolver, `lib` verbs, search, doctor),
  docs + enforcement-matrix row with honest provenance-only runtime cells;
  close the adjacent library `hooks` gap noted in `crates/cli/src/library.rs`.
  - **Landed + reviewed, commit 3b293c1:** `LibraryExtension` +
    git sources through the shared store (strict digest at the checkout
    subpath; offline blocks `--locked`, yellow at trust; rev-drift checked),
    inline-first-then-library resolution with origin labels, lock provenance
    fields (E1-era entries still parse), doctor source+rendered audits,
    search coverage, zero-files exclusion witness, dashboard managed labels,
    docs (reference/ENFORCEMENT/ARCHITECTURE) with provenance-only runtime
    cells, and the library hooks gap closed via the server pattern
    (`lib/hooks/<name>.toml`, `lib add-hook`, `agentstack add <hook>`).
    Renderer delivers all three source kinds from the digest's own anchor
    (`ResolvedExtension::{anchor,declared}`) — witness:
    `library_origin_extension_renders_and_verifies`.
- [ ] E4 (deferred until evidence): unify Claude Code/Codex plugin recipes
  and the guard payloads under the same render engine; Gemini extensions;
  static analysis or capability declarations for extension code.

## Init experience lanes (added 2026-07-17, post-keystone)

**Maintainer scope addition (2026-07-17): make the shipped protection and
secrets machinery visible at first run.** Both lanes are UX over existing
enforcement — no new policy or resolution semantics. Queued behind the
minimum-version keystone review; starting either earlier is a deliberate
scope decision, except S1, which is bugfix-grade and in-cut-sized.

**Details:** [`docs/design/init-access-control.md`](docs/design/init-access-control.md) ·
[`docs/design/init-secrets-experience.md`](docs/design/init-secrets-experience.md) ·
ledger entries D8 and D9 in [`STRATEGY.md`](STRATEGY.md#security-decision-ledger)

### Access control (D8)

- [ ] A0: review and approve the design doc (settle the default deny list
  entry by entry, the guard-offer wording, no-new-verb, and verify whether
  deny globs support home-anchored entries before templating any).
- [ ] A1: extend the `init --global` template with `[guard]` +
  `[policy.filesystem]` defaults; post-write guard-install offer; dashboard
  parity (report, never auto-install). Witnesses per design §7.
- [ ] A2: commented `[policy.filesystem]` block in project init output;
  protection-status line; the two doctor informational findings.
- [ ] A3: "protect this device" docs page; optional fourth demo clip.

### Secrets (D9)

- [ ] S0: review and approve the design doc (settle the three-option prompt,
  `--secrets` flag, `[secrets] default_store`, `secret lift`, and the open
  questions).
- [x] S1 (bugfix-grade, landed in-cut 2026-07-17): interactive init stops
  aborting on an unreachable keychain (stores what it can, reports failed
  refs by name, continues); dashboard init reports unstored refs by name
  instead of dropping silently. Witness:
  `store_lifted_reports_failures_by_name_and_keeps_storing`.
- [ ] S2: the store-choice prompt at the lifting moment, `--secrets`,
  `.env` write path with managed-gitignore verification,
  `[secrets] default_store`.
- [ ] S3: `secret list` plaintext labels, doctor informational finding,
  `agentstack secret lift`, the "Where do secrets live?" docs page.

## Phase 2 — paid design partners, Cloudflare runner, saved workflows

**Start only after Phase 1. Execute the experiments in this order.**

**Details:** [strategy phase 2](STRATEGY.md#phase-2--paid-design-partnerships)

### Design partnerships

- [ ] Sell a limited outcome: governed rollout, policy baseline, or evidence export.
- [ ] Sign the first paying design partner.
- [ ] Turn repeated manual work into product candidates, not bespoke consulting.
- [ ] Sign a second partner with the same core coordination problem.

### Cloudflare one-shot runner

- [ ] Define the hosted adapter contract from the frozen Phase 1 plan.
- [ ] Build one Worker entry point for authentication, policy, approvals, and
  credential brokering.
- [ ] Use one isolated Cloudflare Sandbox for one repository, harness, task,
  frozen grant, returned patch, and receipt.
- [ ] Keep GitHub, model, and long-lived credentials outside the sandbox.
- [ ] Stage only approved read material and validate every returned path,
  symlink, traversal, and write against the grant.
- [ ] Require approval before creating a branch or pull request.
- [ ] Start with bring-your-own Cloudflare and model credentials.
- [ ] Do not add Cloudflare-specific fields to the portable plan or build a
  multi-tenant dashboard in this phase.
- [ ] Prove local and Cloudflare receipts describe the same effective grant.

### Saved governed workflows

**Design banked (2026-07-17):**
[`docs/design/workflows-capability.md`](docs/design/workflows-capability.md)
(ledger D7, W0 review pending). Two directions already settled: no Docker
dependency — the orchestration script runs on an embedded memory-safe
interpreter inside the executor domain, never raw on the host; and `agent()`
takes a `role` (profile) rather than a free-form model/harness, so scripts
request authority and can never widen it. On W0 approval, its W1–W4 stages
replace the sketch items below. The evidence gate (first item) still applies
before W1 starts.

- [ ] Confirm a real repeated task before adding persistence or orchestration.
- [ ] Import and govern a native workflow format before inventing broad syntax
  where practical.
- [ ] Define the restricted, versioned `.agentstack/workflows/*.js` or `*.ts`
  API that normalizes to the frozen plan.
- [ ] Ensure workflow files request authority and can never grant or widen it.
- [ ] Give every role its own profile, folders, tools, secrets, egress,
  commands, budget, and audit identity.
- [ ] Digest-pin workflow source and normalized plans; re-gate trust on change.
- [ ] Never execute arbitrary workflow code on the host.
- [ ] Require idempotency for retried steps and approval or explicit policy for
  irreversible effects.
- [ ] Add Cloudflare Workflows durability only when retries, waits, recovery,
  schedules, or approval events are proven requirements.

### Phase 2 exit gate

- [ ] Two paying partners share the same core coordination need.
- [ ] One partner completes a Cloudflare run matching the local grant contract.
- [ ] One saved role-scoped workflow runs repeatedly without permission widening.
- [ ] Buyer, user, and security stakeholder agree on the value.

## Phase 3 — team control plane

**Details:** [strategy phase 3](STRATEGY.md#phase-3--team-control-plane)

- [ ] Build organization and project inventory.
- [ ] Distribute signed organization policy and Workspace Grants.
- [ ] Add identity, groups, roles, and approval records.
- [ ] Add shared signed workflow distribution.
- [ ] Add fleet health and lock-drift status.
- [ ] Add searchable audit evidence, retention, and export.
- [ ] Integrate existing secret and identity providers.
- [ ] Offer optional metered Cloudflare execution while preserving local and
  bring-your-own-cloud operation.
- [ ] Keep local enforcement functional when the control plane is unavailable.

### Phase 3 exit gate

- [ ] Three organizations use shared policy or audit coordination weekly.
- [ ] One organization expands beyond its pilot team.
- [ ] Support load and infrastructure cost fit a repeatable model.

## Phase 4 — enterprise assurance

**Build only from proven procurement needs.**

**Details:** [strategy phase 4](STRATEGY.md#phase-4--enterprise-assurance)

- [ ] Add SSO, directory synchronization, and delegated administration.
- [ ] Add private networking, regional storage, and deployment choices.
- [ ] Add longer evidence retention and SIEM export.
- [ ] Add formal policy change control and exception workflows.
- [ ] Add requested compliance mappings and audit-ready evidence packages.
- [ ] Define support and reliability commitments.

### Strategic plan completion gate

- [ ] Enterprise sales repeat around the same product and buyer problem.
- [ ] Revenue no longer depends on unrelated custom consulting.
- [ ] Local open-source enforcement and the portable contract remain useful
  without the hosted control plane.

## Deferred until evidence earns them

- Public marketplace or a new MCP directory.
- Broad fleet-management platform.
- Compliance features without a requesting buyer.
- Additional runtime backends without a real deployment requirement.
- Additional agent/client adapters unless requested by real users.
- Repository splits before a component passes the strategy's five-part split test.
- Async proxy event-sink optimization unless measured latency justifies moving
  the current synchronous append off the async path; this is tracked debt, not
  a correctness gap.
