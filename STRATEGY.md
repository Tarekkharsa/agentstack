# AgentStack product strategy

> **Status:** source of truth for product direction and sequencing
>
> **Current as of:** AgentStack 0.15.x
>
> **Updated:** 2026-07-23
>
> **Audience:** maintainers, contributors, design partners, and future collaborators

## Executive decision

AgentStack will become the **vendor-neutral environment manager for AI coding
tools**.

The product promise is:

> **Define your agent setup once. Use it across every coding CLI.**

AgentStack imports, organizes, activates, diagnoses, restores, and shares the
skills, MCP servers, instructions, extensions, and profiles developers use
across Claude Code, Codex, Cursor, Gemini CLI, OpenCode, and other harnesses.

Its security architecture remains non-negotiable, but security is the
foundation of the product rather than the first problem every user must care
about. Users should first experience less repeated configuration, cleaner
project switching, and confidence about what changed. Trust, policy, locking,
confinement, and evidence make that convenience dependable.

The operating decisions are:

1. Lead with cross-CLI portability and configuration lifecycle management.
2. Make t3code the primary graphical product surface and launch channel; keep
   the CLI as the authority, automation contract, and complete power-user path.
3. Make setup through t3code or `init → apply → doctor` the shortest path to
   first value.
4. Make profiles and reversible sessions the primary recurring-use feature.
5. Elevate `doctor`, `diff`, `adopt`, and `restore` into core product
   experiences, not supporting utilities.
6. Use progressive disclosure: preserve strong defaults while showing safety
   concepts only when the user's action makes them relevant.
7. Keep security guarantees and their witnesses intact while simplifying how
   users encounter them.
8. Freeze new capability categories until the primary journey is validated
   with people other than the maintainer.
9. Keep workflows experimental until the ordinary configuration product has
   repeated use and the workflow boundary has completed independent review.
10. Extract the authority data path from the large CLI crate along its existing
   security seams; do not split code merely to reduce line counts.
11. Do not build a hosted runner, enterprise control plane, public registry, or
   generic workflow platform until observed usage earns one.
12. Use evidence from real users to decide what comes after the local product.

## What the product is

AgentStack is one control layer over the fragmented configuration surfaces of
agent-enabled developer tools.

```text
existing CLI configurations
           │
           ▼
      import / init
           │
           ▼
  one manifest + lockfile
           │
     ┌─────┼──────────┐
     ▼     ▼          ▼
  apply  session   zero-files
     │     │          │
     └─────┼──────────┘
           ▼
 Claude · Codex · Cursor · Gemini · OpenCode · …
           │
           ▼
 doctor · diff · adopt · restore · activity
```

The manifest is the source of truth. Native CLI files are compiled output.
Profiles select the capability set for a project or task. Delivery modes decide
whether that set is rendered permanently, activated temporarily, or served
dynamically. The lifecycle commands explain and reconcile what happened.

Security surrounds this flow:

```text
portable configuration
        │
        ├── lock: what exact content was selected?
        ├── trust: did the user approve this project state?
        ├── policy: what is this machine willing to allow?
        ├── gateway/runtime: where is that decision enforced?
        └── recorder: what actually happened?
```

## Product surfaces

AgentStack has one product with two interfaces, not two competing products.

### t3code: the primary graphical experience

t3code is the preferred surface for discovery, onboarding, everyday status,
toolset selection, temporary activation, recovery, and contextual safety
guidance. It matters strategically because users can receive AgentStack's value
inside the environment where they already launch and supervise coding agents.
They should not need to learn a second application before seeing the benefit.

The first t3code experience should answer:

1. What coding tools and existing capabilities did AgentStack find?
2. Which named toolset do I want for this project?
3. Is it ready?
4. What safe action fixes the current problem?
5. How do I undo a change?

t3code is not trusted to invent commands, resolve workspace paths, or enforce
consent. It consumes versioned read schemas and invokes a closed enum of
server-owned actions that map to fixed CLI argv. The CLI independently checks
every precondition. A frontend bug can make the UI wrong or unavailable; it
must not grant more authority.

t3code's MCP surface may later provide useful launch and supervision plumbing
for workflows that call other coding harnesses. Treat that as an integration
opportunity, not a second authority system: AgentStack must freeze and admit
the child execution plan first, and every t3code-mediated launch must remain
the same governed child-run path with the same evidence and cancellation
semantics. This stays experimental until the real MCP tool surface is
documented and the no-bypass property has witnesses.

### CLI: authority, automation, and the complete path

The CLI remains fully usable on its own. It owns parsing, validation, dry runs,
writes, trust, policy, activation, recovery, and runtime enforcement. It also
provides the stable JSON and action contracts used by t3code, CI, MCP, and
future integrations.

The embedded AgentStack dashboard is retired. Maintaining a second local UI
would divide product attention, duplicate t3code, and invite contract drift.
Graphical product work goes into t3code; reusable behavior goes into the CLI
and its machine-readable interfaces.

## The problem

Developers increasingly use more than one coding agent. Each tool invents its
own configuration format, file locations, installation model, and terminology
for the same underlying capabilities.

This creates recurring problems:

- The same MCP server is copied into several incompatible configuration files.
- Skills and instructions are manually duplicated and drift independently.
- Global configuration leaks into projects that do not need it.
- Switching between tasks requires editing shared files and remembering how to
  undo the change.
- A teammate cannot reproduce another developer's working setup reliably.
- Multiple tools write the same native files without a shared ownership model.
- Users cannot easily answer what is installed, active, broken, duplicated, or
  stale.
- Repository-supplied configuration can execute code or receive secrets before
  the user understands its surface.

The first six are visible every day. The last one is less visible but more
consequential. AgentStack must solve both without requiring users to begin with
a security education.

## Target users and jobs

### Primary: developers using two or more agent CLIs

Their jobs:

- Import the setup they already have.
- Stop maintaining the same capability in multiple formats.
- Keep project and global configuration understandable.
- Move between tools without rebuilding the environment.
- Recover safely when a configuration change is wrong.

Their success moment:

> “I changed one manifest and all of my coding tools now have the same setup.”

### Secondary: developers with different stacks per project or task

Their jobs:

- Define a minimal profile for each context.
- Start a temporary working session.
- End the session and restore native files exactly.
- See which profile is active and whether it is ready.

Their success moment:

> “I switched from my backend stack to my incident-response stack without
> editing or cleaning up five config files.”

### Third: teams sharing an agent environment

Their jobs:

- Commit a portable, secret-free manifest and lockfile.
- Onboard another machine predictably.
- Share reviewed skills, servers, instructions, and profiles.
- Detect drift before it becomes a support problem.
- Apply a machine or organization ceiling that repository configuration cannot
  loosen.

Their success moment:

> “A teammate cloned the project, supplied their own secret values, and reached
> the same working environment.”

### Advanced: developers automating repeated multi-agent work

Their jobs:

- Save a reviewed orchestration script.
- Bind each role to a predefined profile.
- Run and resume it without widening authority.
- Understand its steps and evidence afterward.

This is a real differentiator, but it is not the beginner path.

## The value ladder

### 1. Acquisition: unify what already exists

The first promise is:

> **Your current agent configuration, imported once and rendered everywhere.**

The acquisition journey is:

```text
install → init → review imported setup → apply → doctor
```

It should take less than five minutes for a typical developer with at least two
supported CLIs. It should not require Docker, policy authoring, a gateway, a
library, or an understanding of delivery modes.

Users may complete this journey in t3code or the terminal. Both paths use the
same CLI planning and write contracts and must produce the same result.

### 2. Retention: switch and repair confidently

Profiles, sessions, diagnostics, reconciliation, and recovery create recurring
value:

```text
choose profile → start session → work → inspect → end/restore
```

The product should make these questions easy:

- What is active?
- Which project or profile owns it?
- Is it pinned and ready?
- What differs from the manifest?
- Will this command keep or replace a hand edit?
- How do I return to the prior state?

### 3. Expansion: share and automate

Once developers depend on AgentStack locally, expansion can include:

- Team manifests and profiles.
- Reusable library packages.
- Signed or approved internal sources.
- Organization policy baselines.
- Evidence export.
- Governed workflows.
- A hosted coordination layer if real teams require it.

Expansion must grow from repeated local behavior. It must not be guessed from
what the architecture could theoretically support.

## Product pillars

### Unify

One manifest describes skills, MCP servers, instructions, extensions, targets,
and profiles across supported CLIs.

Product requirements:

- Import existing native configuration accurately.
- Preserve secret values outside the manifest.
- Render deterministic native output.
- Explain unsupported or lossy fields before writing.
- Keep adapter behavior testable through shared conformance fixtures.

### Activate and switch

Users choose how a profile reaches a CLI:

- **Apply:** stable native files for normal offline launches.
- **Session:** temporary native activation with exact restoration.
- **Zero-files:** live delivery when the client can consume capabilities
  dynamically.

These are implementation modes, not three products. User interfaces should ask
what outcome the user wants and recommend a mode rather than presenting all
three concepts at once.

### Inspect and repair

`doctor`, `diff`, `adopt`, `apply`, and `restore` form one lifecycle:

- `doctor` identifies the problem.
- `diff` shows the consequence of a change.
- `adopt` keeps an intentional native edit by bringing it into the manifest.
- `apply` makes native output match the manifest.
- `restore` returns to a previous recorded state.

Every diagnostic should name the next safe action. Every write should have a
preview or a recovery path.

### Organize and reuse

Profiles create task-specific environments. The library and packs make
capabilities reusable across projects without copying their definitions.

The product should prefer a small number of understandable composition
concepts:

- Manifest: this project's source of truth.
- Profile: a named subset for a task or role.
- Library package: reusable content resolved and pinned into a project.

Other internal nouns should stay out of the beginner experience.

### Share

A shareable AgentStack project contains declarations, references, and pins—no
secret values. Another machine supplies its own secret resolution and applies
its own policy ceiling.

Team sharing is successful when two people can reproduce the same capability
surface without sharing credentials or manually reconciling native files.

### Automate

Workflows compose existing profiles into repeated tasks. They request
pre-reviewed authority; they never create authority themselves.

Workflows remain experimental until:

- Module loading and all other script-boundary paths are fail-closed.
- The engine has completed independent boundary review.
- At least three recurring tasks demonstrate continued use.
- The ordinary profile/session product is already understandable.

## Security’s role

Security is a product quality and a differentiator. It is not the only
acquisition message.

| User-visible feature | Security underneath |
| --- | --- |
| One manifest across CLIs | Hostile repository declarations remain inert until trusted |
| Reusable library content | Locking detects content drift |
| Profiles | Machine policy remains the upper bound |
| Temporary sessions | Ownership and restore records prevent silent state loss |
| Secret references | Values stay machine-local and unresolved failures stop writes |
| Workflows | Frozen grants and role binding prevent authority growth |
| Activity and reports | Evidence records what went through governed paths |

The non-negotiable invariants remain:

1. Policy can only narrow.
2. Untrusted means inert.
3. Any pinned byte change re-gates.
4. Secrets never serialize into the manifest or lockfile.
5. Authority has one construction path.
6. Upstream tool dispatch has one gateway path.
7. Claims match the enforcement matrix for each mode.

Security copy should be concrete and placed near the action it protects. The
full threat model and enforcement matrix remain available for users who need
them, without dominating the first-run experience.

## User experience strategy

### Progressive disclosure

Strong safety does not require front-loading every safety concept. The beginner
journey exposes only:

- Setup
- Toolset
- Status
- Undo

The disclosure ladder is:

| Moment | What the user sees | What remains underneath |
| --- | --- | --- |
| First launch | Detected tools, imported capabilities, recommended setup | adapter details, ownership ledger, lock mechanics |
| Normal local use | Toolset, readiness, changed files, undo | machine ceiling and fail-closed validation |
| Unfamiliar project content | “Review this project” with the exact surface and consequence | content-bound trust digest |
| A real denial | What was blocked, why, and one safe next action | matching policy or guard rule |
| User asks for stronger isolation | “More protection” choices with cost and coverage | gateway, sandbox, egress, confinement details |
| Audit or investigation | Activity and honest coverage labels | record formats and enforcement matrix |

This leads to six interface rules:

1. **Value before vocabulary.** Say what the user can accomplish before naming
   the internal primitive.
2. **No decision without a consequence.** Apply safe defaults when there is no
   meaningful choice; do not make users configure security for its own sake.
3. **Just-in-time boundaries.** Explain trust, pins, policy, and confinement at
   the action where each changes the outcome.
4. **A block must be recoverable.** Every denial says what happened, what is
   protected, and the exact safe next step. Never stop at “policy denied.”
5. **Recommended path first.** Advanced controls live behind “More protection,”
   “Details,” or the equivalent—not beside the beginner action with equal
   weight.
6. **Honesty remains visible.** Simplified language must not imply enforcement
   the selected mode does not provide.

The CLI may retain precise power-user verbs, but documentation and t3code
organize them by outcome:

| User intent | Primary action |
| --- | --- |
| Set up this project | `agentstack init` |
| Make my CLIs match | `agentstack apply --write` |
| Check the setup | `agentstack doctor` |
| Use a task-specific environment | profile + session |
| Understand a difference | `agentstack diff` |
| Keep my manual edit | `agentstack adopt --write` |
| Undo a change | `agentstack restore` |

Trust, locks, sources, policy, gateways, and confinement appear when the user
shares, imports unfamiliar content, or asks for stronger execution guarantees.

Progressive disclosure must never become progressive enforcement. Safe defaults
and fail-closed checks run from the beginning; only their explanation is
deferred until relevant.

### One recommended path

Documentation must present one default before alternatives:

1. Install.
2. Run `agentstack init`.
3. Review what was imported.
4. Apply it.
5. Run `agentstack doctor`.

Only after success should the product introduce profiles and sessions.

In t3code, the same journey should be a short guided flow backed by
`init --plan`, explicit review, a fixed apply action, and status. The UI may
combine steps visually, but it may not bypass the CLI's previews or consent
requirements.

### Explain writes before performing them

For every material write:

- State which files will change.
- Distinguish managed entries from foreign entries.
- Preview destructive or replacing behavior.
- Provide the recovery command.
- Keep client-supplied paths and command lines out of UI RPCs.

## Positioning and messaging

### Primary message

> **One agent setup. Every coding CLI.**

Supporting line:

> Import the tools, skills, and instructions you already use; switch them by
> project or task; diagnose drift; and restore changes safely.

Trust line:

> Portable does not mean automatic: unfamiliar repository configuration stays
> inert until you review it, and machine policy always wins.

### Proof sequence

The homepage, README, demo, and first-run experience should prove value in this
order:

1. Detect existing CLIs and configurations.
2. Import one existing MCP server.
3. Render it into at least two native formats.
4. Show a clean `doctor`.
5. Switch to a second profile temporarily.
6. Show `diff` and `restore`.
7. Explain trust and stronger enforcement.

### Language to avoid as the opening

Do not lead with:

- “Security tool”
- “Trust and policy plane”
- “Governed execution”
- “Agent bundle standard”
- “Enterprise control plane”

Those descriptions are accurate in deeper contexts but force a new user to
understand the implementation before recognizing the benefit.

## Differentiation

Native CLI vendors can improve their own trust gates, configuration UI, and
plugin systems. They are unlikely to provide a neutral lifecycle manager for
their competitors.

AgentStack’s defensible combination is:

- Cross-vendor normalization.
- Import and deterministic rendering.
- Profiles spanning different clients.
- Reversible activation and ownership-aware reconciliation.
- Reproducible content locking.
- Consistent trust and machine ceilings.
- Comparable evidence across governed paths.

No single feature is the moat. The durable value is one coherent lifecycle
across otherwise incompatible tools.

## Scope and non-goals

AgentStack should own:

- The portable manifest and lock contract.
- Imports and adapters for supported CLIs.
- Profiles and activation lifecycle.
- Drift diagnosis, reconciliation, and restoration.
- Capability provenance and reusable library resolution.
- Trust, policy, gateway enforcement, confinement, and evidence.
- Governed workflow execution after the core product is validated.

AgentStack should integrate with:

- Existing agent CLIs rather than replacing them.
- t3code as the primary graphical launch and supervision surface.
- Vendor and community capability catalogs.
- OS and external secret stores.
- Existing sandbox and runtime technologies.
- CI and source-control systems.

AgentStack will not build now:

- Another general-purpose coding agent.
- A public MCP or prompt marketplace.
- A generic workflow engine.
- A hosted multi-tenant runner.
- A Cloudflare-specific product.
- An enterprise administration suite.
- Background scheduling and durable jobs.
- Separate repositories for components without independent adoption.
- A replacement for t3code or another embedded dashboard.
- New capability categories before the current ones are validated.

These are evidence-gated possibilities, not roadmap commitments.

## Engineering strategy

### Preserve the working foundation

The manifest, lock, trust, policy, adapter, recorder, runtime, egress,
executor, workflow, and CLI code are shipped foundations. Product
simplification does not authorize weakening or rewriting their established
invariants.

### Extract the authority kernel along existing seams

The relevant path is:

```text
AuthorityGrant construction
        ↓
ExecutionPlan
        ↓
Gateway::try_call
        ↓
secret resolution and upstream transport
```

Extraction succeeds when:

- There is still one constructor that can mint frozen authority.
- There is still one path to an upstream transport.
- Boundary types such as `CompiledRuleset` and `GrantHandoff` remain explicit.
- The moved crates forbid unsafe code.
- Existing narrowing, pin, trust, and dispatch witnesses remain green.
- The CLI becomes an orchestration shell rather than an alternate authority
  implementation.

A smaller crate is an effect, not the acceptance criterion.

### Freeze capability breadth

Until activation is proven:

- Fix confirmed correctness and security findings.
- Improve the primary journey.
- Harden existing adapter and lifecycle behavior.
- Extract review boundaries.
- Do not add new capability kinds or execution backends.

### Treat workflows as an advanced lane

The immediate workflow obligations are correctness:

- Explicitly disable Boa filesystem module loading.
- Keep dynamic string compilation disabled.
- Bound loops, recursion, stack, wall time, and outputs honestly.
- Preserve the out-of-thread watchdog.
- Complete independent script-boundary review.

Further authoring UI, durability, scheduling, cloud execution, or library
distribution waits for product evidence.

One bounded workflow research track is allowed: evaluate t3code's MCP as an
optional launch and supervision backend for other harnesses. It must sit behind
the existing governed child-launch seam, not introduce another authority
constructor or transport path. The direct CLI launcher remains the baseline
and fallback. No beginner-facing promise depends on this research.

## Roadmap and gates

The exact executable queue lives in [`TODO.md`](TODO.md). These stages describe
outcomes, not parallel feature lanes.

### Stage 0 — close confirmed correctness gaps

Outcome: no known fail-open boundary blocks the new product journey.

Required:

- Disable Boa’s default filesystem module loader and add a witness.
- Complete immutable consent-snapshot review and tests.
- Finish the t3code consent-digest plumbing before enabling UI trust writes.
- Keep the current UI write path fail-closed until its admin authorization
  boundary is explicit.

Gate: focused tests and line-by-line review of the affected security paths.

### Stage 1 — make first value obvious

Outcome: a new user can unify an existing setup in under five minutes from
t3code or the terminal.

Required:

- Rewrite public positioning around cross-CLI portability.
- Make the t3code setup experience and `init → apply → doctor` two views of one
  coherent CLI-owned journey.
- Ship a t3code setup/status slice backed by `init --plan`, fixed actions, and
  versioned schemas.
- Show exactly what was imported and where it will be rendered.
- Produce one reproducible demo using at least two CLIs.
- Test the journey with five people who did not build AgentStack.

Gate:

- At least four of five users finish without maintainer intervention.
- Median time to a clean `doctor` is below five minutes.
- Users can explain the product as “one setup across my agent CLIs.”
- No test participant needs to understand trust, policy, gateway, or sandbox
  terminology before a relevant boundary appears.

### Stage 2 — make profiles the recurring habit

Outcome: users can switch between two capability sets and restore cleanly.

Required:

- Make profile listing and readiness understandable.
- Document one profile creation path.
- Make temporary session start/end observable and recoverable.
- Show active profile and ownership in status/doctor surfaces.
- Add the toolset picker to t3code as soon as the CLI contract is stable.

Gate:

- Three users create or select two profiles.
- They can start and end a session without manual native-file cleanup.
- At least two use profiles again in a later session.

### Stage 3 — make lifecycle confidence a product feature

Outcome: users rely on AgentStack to understand and repair configuration drift.

Required:

- Connect doctor findings to diff/adopt/apply/restore actions.
- Verify ownership behavior across the most-used adapters.
- Make recovery instructions visible before writes.
- Test hand edits, foreign entries, overlapping projects, and interrupted
  sessions through user-facing scenarios.

Gate:

- Five drift scenarios produce a correct diagnosis and safe next action.
- Users recover from an intentional bad change without inspecting internal
  state files.

### Stage 4 — validate sharing

Outcome: another person reproduces a project’s environment without receiving
secret values.

Required:

- Simplify team onboarding documentation.
- Prove manifest/lock portability across two machines.
- Validate library/package reuse with real shared content.
- Identify whether teams need signing, policy distribution, or hosted
  coordination before building them.

Gate:

- Three independent project handoffs succeed.
- At least one shared package is reused in two projects.
- Repeated team pain identifies the next product investment.

### Stage 5 — earn advanced expansion

Possible directions:

- Governed workflow authoring and supervision.
- Organization policy distribution.
- Evidence export and compliance integrations.
- Hosted coordination or execution.
- A package registry.

No direction enters the roadmap without observed repeated demand and a narrow
first use case.

## Metrics

### Activation

- Installation success rate.
- Time from install to first imported manifest.
- Time to first successful apply.
- Time to a clean doctor result.
- Number of native configurations imported.
- Number of target CLIs rendered.

### Retention

- Profiles selected per week.
- Temporary sessions started and ended cleanly.
- Doctor/diff invocations after initial setup.
- Restore and adopt outcomes.
- Projects managed per user.

### Expansion

- Manifests shared across machines or people.
- Library packages reused across projects.
- Team policy baselines applied.
- Workflows run repeatedly rather than once.

Telemetry must be opt-in or collected through explicit user studies until a
privacy-preserving measurement design is approved.

## Risks and responses

| Risk | Response |
| --- | --- |
| Users think AgentStack is only for security teams | Lead with configuration portability and show security as the foundation |
| Too many concepts create a steep learning curve | Present one recommended journey and reveal advanced modes progressively |
| Restrictions make users abandon setup | Keep strong defaults, remove premature decisions, and make every block explain the exact safe next step |
| Capability breadth overwhelms one maintainer | Freeze new lanes and prioritize activation, retention, and extraction |
| Native vendors absorb individual features | Own the cross-vendor lifecycle rather than one feature |
| Adapter behavior becomes inconsistent | Shared descriptors, conformance fixtures, and priority based on real usage |
| Profiles remain a power-user abstraction | Provide task-based examples, readiness, active state, and reversible sessions |
| Workflows distract from the core product | Keep them experimental and evidence-gated |
| Security work consumes all product attention | Separate invariant maintenance from public positioning and product milestones |
| t3code fork maintenance becomes a drag | Keep the integration thin, schema-versioned, and CLI-owned; upstream the generic seams and avoid UI-only business logic |
| CLI and t3code drift into separate products | One acceptance suite must prove both surfaces produce the same planned changes and outcomes |
| Product work weakens enforcement | Security witnesses remain release gates regardless of roadmap stage |

## Operating discipline

Every proposed task must answer:

1. Which user and job does this serve?
2. Is it acquisition, retention, expansion, or foundation?
3. What observable outcome will prove it helped?
4. Does it introduce a new concept or capability lane?
5. Can existing functionality solve the problem more simply?
6. Which security invariants or authority seams does it touch?

If a task cannot answer the first three, it does not enter the active roadmap.
If it introduces a new lane before Stage 4 evidence, it is deferred.

## Definition of success

AgentStack succeeds when developers describe it first as:

> “The tool that keeps my agent setup consistent across all my coding CLIs.”

They should discover afterward that it also:

- Keeps unfamiliar repository configuration inert until reviewed.
- Prevents project policy from exceeding the machine ceiling.
- Detects changed or unpinned content.
- Provides stronger governed execution when needed.
- Records honest evidence about the paths it actually controls.

Convenience earns adoption. Lifecycle confidence earns retention. Security
makes both trustworthy.
