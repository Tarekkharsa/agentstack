# AgentStack execution checklist

> **Status:** active work queue<br/>
> **Updated:** 2026-07-15<br/>
> **Current phase:** Phase 0A and Phase 0B in parallel<br/>
> **Strategy source:** [`STRATEGY.md`](STRATEGY.md)

This is the only ordered day-to-day plan. Start with the first unchecked item
in the current phase. Do not begin a later phase until the current phase's exit
gate is satisfied. `STRATEGY.md` explains why and defines the gates;
`docs/ARCHITECTURE.md` and `docs/ENFORCEMENT.md` define the technical boundaries.

## How to work from this file

1. Select the first unchecked item in Phase 0A or the parallel Phase 0B lane.
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

- [ ] Write the reviewed behavioral contract for
  `agentstack run <harness> --locked` using the current working directory as
  the project.
- [ ] Define how `--locked` composes with `--profile`, `--plan`, the required
  harness positional, and trailing harness arguments.
- [ ] Define the exact no-Docker guarantee: explicit trust, locked-input and
  drift checks, machine-policy ceiling, cooperative host guard, evidence, and
  honest non-isolation limits.
- [ ] Define the maximum-assurance guarantee separately for `--sandbox` and
  `--lockdown`.
- [ ] Freeze the minimum backend-neutral fields needed later by Workspace
  Grants and hosted adapters without implementing those later phases now.
- [ ] D2: specify how the existing `ExecutionPlan` / `Gateway::from_plan` seam
  extends across native render and profile leases without a parallel authority
  abstraction.
- [ ] D3: decide which stdio scripts and local executables are declared
  integrity inputs for `--locked`; if the first release excludes them, write
  the limitation into the contract and trust preview.

### Implementation

- [ ] Implement the canonical `run <harness> --locked` flow.
- [ ] Require trust before any repository-controlled hook, tool, server, or
  secret can activate.
- [ ] Resolve locked inputs and fail before activation on missing pins, drift,
  or unverifiable state.
- [ ] Compile repository policy beneath the machine-policy ceiling.
- [ ] Launch through the canonical gateway path and preserve the documented
  assurance label for the selected mode.
- [ ] Record material trust, policy, capability, lifecycle, and outcome
  decisions without recording secret values.
- [ ] Complete the remaining recorder events for trust-store mutations and
  per-run token/cost evidence.
- [ ] Extend the D2 frozen grant across render, gateway, and profile leases:
  resolved identities, verified pins, trust, effective policy, secret grant and
  lifetime, confinement posture, and evidence identity.
- [ ] Implement D3 lock digests and trust-review display for declared local
  executable inputs; make `doctor` warn on executable-but-unpinned code.
- [ ] Resolve the unreleased `agentstack_lease_freeze` →
  `agentstack_lease_capture` naming decision before external compatibility; if
  accepted, rename directly with no alias or migration shim.

### Proof and activation

- [ ] Demo: safe repository, standard binary, no Docker.
- [ ] Demo: machine policy blocks a repository-requested capability, no Docker.
- [ ] Demo: changed or missing locked input fails before activation, no Docker.
- [ ] Demo: maximum-assurance sandbox and lockdown behavior with Docker.
- [ ] Add claim-consistency tests for the enforcement matrix and recorded event list.
- [ ] Prove every delivery path consumes the same D2 grant and cannot
  independently reconstruct or widen authority.
- [ ] Prove a one-byte D3 executable edit fails lock verification and re-gates
  review; prove intentionally unpinned code is labelled honestly.
- [ ] Add a regression witness that authoritative trust/lock verification hashes
  current bytes and never uses the stat-fingerprint digest cache.
- [ ] Add documentation status labels for Stable, Experimental, Design,
  Historical, and Archived material.
- [ ] Generate the public strategy page from authoritative Markdown or add a
  synchronization check that prevents phase/status drift.
- [ ] Extend documentation claim tests to CLI examples, versions, adapter
  counts, and feature status—not only matrix cells and event names.
- [ ] Label every public example as a tested fixture, Docker reproduction,
  validated manifest, or illustrative snippet.
- [ ] Observe five strangers completing the protected run without maintainer help.
- [ ] Reach a median time to first protected run below 15 minutes.
- [ ] Update the README and website so every claim matches the demonstrated tier.

### Phase 0A exit gate

- [ ] All three no-Docker demos and the separate Docker demo pass.
- [ ] Five unassisted users complete the flow.
- [ ] Median activation is below 15 minutes.
- [ ] Documentation and enforcement claims agree.

## Phase 0B — validate the problem and distribution

**Runs in parallel with Phase 0A.**

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
