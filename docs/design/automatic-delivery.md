# Automatic delivery: dynamic-first, rendered where required

> **Status:** Adopted 2026-08-02 by maintainer decision, after an adversarial
> review of the "dynamic by default" proposal. This document is the in-repo
> authority for the decision; the review memo it came from is a session
> scratchpad artifact and is **not** in the repository, so nothing may be
> resolved by citing it. Where the memo and this document differ, this document
> is right.
>
> **Direction:** [`STRATEGY.md`](../../STRATEGY.md) — "Delivery ambition"
> (dynamic zero-files as the eventual default, with named preconditions) and
> open design question #1 (where the yes lives in zero-files mode). This
> document answers question #1 and fixes the contracts the flip needs; it does
> not amend the strategy and does not authorize work on its own.
>
> **Ordered work:** [`TODO.md`](../../TODO.md) — "Automatic delivery
> (package-aware, dynamic-first)".
>
> **Vocabulary:** Package · Toolset · Lease. Defined in §Package-aware delivery
> and used consistently below.
>
> **Amended 2026-08-02 (STRATEGY.md v3 adoption):** dynamic becomes the
> default when the workstreams land (arc-end) rather than behind a
> release-sequenced flip; the advanced override set is reduced to **Render
> locally** ("Prefer gateway" removed); and the §1.6 activation study is
> removed from the flip's preconditions — it runs when v3's bar is met and
> no longer gates delivery.
>
> **Landed 2026-08-03 (W4, arc-end): the default is dynamic.** All seven
> remaining preconditions were verified before the flip; the evidence is
> recorded in §The preconditions for the flip. The delivery planner ships in
> `crates/cli/src/delivery.rs`, the override as `[delivery] render_locally`
> (per project or per harness, `agentstack delivery render-locally`), and the
> routing is read through `agentstack delivery [--json]`
> (`delivery-routing-v1`). Everything below this line is the decision as
> adopted; the flip did not amend any of it.
>
> **Corrected 2026-08-03 (item 4, instruction variants): "MCP cannot inject
> these" was not true.** The delivery matrix below justified routing
> instructions to the rendered lane with a claim about the MCP protocol. The
> protocol has a purpose-built `initialize` `instructions` field, AgentStack's
> own gateway already populates it, and Claude Code is confirmed to consume it
> ([`research/dynamic-instructions-2026-08.md`](research/dynamic-instructions-2026-08.md)).
> **The lane is unchanged; the justification is replaced** with the accurate,
> narrower, per-harness one: no live channel a harness is *known* to consume
> can carry an instruction **per model** or **behind a lease** — `initialize`
> carries only a client name and version, and it fires before any toolset is
> selected. The full argument, the per-harness confirmation matrix, and the
> variant schema are in
> [`instruction-variants.md`](instruction-variants.md). Nothing else here
> changes: instructions still render, and no surface describes one as going
> live "via gateway".

## The decision

AgentStack ships a **delivery planner**: for each capability, it chooses a
delivery lane from the capability's *kind* and the *harness* it is going to.

- **Dynamic lane** — skills and MCP servers on MCP-capable harnesses, served
  through the gateway lease: brokered, policy-checked, digest-verified,
  recorded.
- **Rendered lane** — instructions, settings, hooks, extensions, and *every*
  capability on a harness without MCP: written into native files exactly as
  today.

This is **not a mode switch, and static rendering is not being removed** — it
is being *routed*. It stays the only correct answer for what no live channel
can carry correctly (corrected 2026-08-03 — see the amendment above) and for
harnesses that cannot take a gateway. A project can be, and normally will be,
in both lanes at once.

The user setting is **Automatic** by default; the planner runs silently and
status names what happened. One advanced override exists (amended 2026-08-02)
behind the "More control" path, settable per project or per harness:

- **Render locally** — write files even where the lease would work.

The override exists because the reasons to want files are real, and they are
enumerated so nobody has to re-argue them: offline operation; deterministic
native files; inspection with ordinary filesystem tools; corporate policy
prohibiting a persistent daemon; debugging without another runtime dependency;
and compatibility testing against native CLI behaviour. Removing the choice
would make the system harder to recover when automatic routing is wrong.

### Delivery matrix

| Capability kind | Lane | How it is delivered |
|---|---|---|
| Skills | dynamic | Gateway, on demand — digest-verified per load |
| MCP servers | dynamic | Gateway lease — brokered, policy-checked, recorded |
| Instructions (managed `CLAUDE.md` / `AGENTS.md` region) | rendered | Rendered file — no live channel a harness is *known* to consume can carry one per model or behind a lease (corrected 2026-08-03; the original "MCP cannot inject these" was disproven — see the amendment above and [`instruction-variants.md`](instruction-variants.md)) |
| Settings | rendered | Rendered into native config |
| Hooks · Extensions | rendered | Rendered; full consent ceremony always (executable kinds) |
| Any kind, non-MCP harness | rendered | Full static delivery, automatically |

A project in the dynamic lane carries `.agentstack/` — manifest plus lock, the
committed source of truth — and, when instructions are used, one managed region
in the instruction file. No `.mcp.json`, no `.claude/skills/` symlinks, and no
gitignore block for what was never written.

## Where the yes lives

This section answers `STRATEGY.md` open design question #1.

> **The gateway serves nothing to an unreviewed or drifted project. The review
> card renders in the CLI or the panel, never as an MCP tool the agent can
> invoke. A byte change freezes new serving and raises the diff card.**

Consent stays exactly what it is today — content-bound, per project, one
deliberate yes. What moves is its *trigger* and its *rendering*: a refused lease
or load becomes an event the CLI and the panel surface as a yes-card prompt
("Needs your yes"), using the same card as `agentstack trust` and the panel
(`trust-review-card-v1` / `trust-card-diff-v1`, per
[`consent-card.md`](../archive/design/consent-card.md) (archived; cited
normatively)). The refusal must be *loud* — it names
what was refused and the one command that fixes it — and it must never be
answerable in the agent's channel, because content the gate exists to govern
can forge anything that travels in that channel.

Nothing here widens the gate: a lease is not a consent path, it is a delivery
path that a prior consent authorizes.

## Security model

### Trust is checked at dispatch, from the digest

**Every gateway dispatch to an upstream capability compares the current consent
digest.** A generation token may cache and accelerate that comparison, but it is
**never authoritative**: `git pull`, a manual edit, and a lock replacement all
happen outside AgentStack and would never bump an in-memory counter. Any
filesystem change, any watcher failure, and any uncertain state forces digest
recomputation, and an inconclusive recomputation fails closed.

The consequence is the honest one: revoking trust, or drifting the pinned bytes,
must stop the *next* upstream call on an already-established connection — not
merely refuse the next lease, load, or session. What is emptied is the
**upstream capability surface**: the leased servers' tools and the loadable
skills go away. **Control-plane tools stay available**, because a user whose
project just went untrusted needs to see why and fix it — blinding the surface
completely would turn a fail-closed refusal into a dead end.

**This is the release blocker of the lane.** Today, ordinary upstream tool calls
never re-check trust: an already-spawned server stays proxied until the next
load, lease, or session call. The dynamic default cannot be the default while a
revoked yes leaves a live path open, and no other workstream substitutes for it.

The fix tightens the existing dispatch path. It adds no second authority
constructor and no second upstream transport (invariant 6).

### The reproducibility rule

> **Runtime resolves from the project lock and serves the pinned bytes from the
> content-addressed store by digest — never from the mutable current state of
> the library.**

This is the property that makes updates non-interrupting: the library can move
ahead arbitrarily far without changing, breaking, or interrupting any project,
because no project ever reads it at serving time. It is also what makes the
compact package reference in §Package-aware delivery safe.

## Update model

Package-manager semantics, stated as four rules:

1. **`lib sync` announces; it never re-gates and never interrupts.** Pulling new
   library versions changes nothing in any project. Library-sourced members are
   pinned at lock like everything else, and pinned bytes are immutable in the
   content store.
2. **`status` offers.** It names that updates are available and the one command
   that takes them.
3. **Upgrading is explicit, per project, on the user's schedule.** It ships
   today as `lock --upgrade <pack>`; a friendlier `upgrade` verb is a *working
   name* only, and the shipped surface wins until it is renamed deliberately.
   The flow shows the aggregate package diff first (version → version, counts by
   member kind), then the per-member diffs, through the same review card.
4. **Keep-pinned is the resting state.** Declining an upgrade is a complete,
   stable answer, not a deferral — per
   [`consent-card.md`](../archive/design/consent-card.md) (archived; cited
   normatively), keep-pinned keeps the approved bytes actually in use.

### Mixed-lane upgrades are transactional, and report per lane

A package upgrade whose members span both lanes — say two skills and one
instruction — is one transaction: it updates the lock **and** re-renders the
managed instruction region, or it does neither. The report then names each lane
separately:

```text
✓ upgraded rust-backend v2.3.1 → v2.4.0
  dynamic lane: 2 skills re-pinned — live via gateway now
  rendered lane: instruction region updated in CLAUDE.md (this project)
```

**An instruction is never described as going live "via gateway".** It went to a
file; the sentence must say so. This is a binding copy rule, not a suggestion —
a single blended success line is how a user comes to believe no file was
touched when one was.

> **The example's dynamic-lane wording, settled 2026-08-03.** The literal
> `2 skills re-pinned — live via gateway now` above is illustrative and is
> **not** printed, conditionally or otherwise. `upgrade` performs a *pinning*
> act; whether those exact bytes are being served is an *activation* fact that
> depends on the bridge being registered, the project being trusted at its
> current bytes, and a lease selecting a toolset containing the skill — none of
> which this command touches — and a project may have Render locally set, in
> which case the bytes go to a file. The shipped line states what is true in
> every case: `dynamic lane: 2 skills re-pinned — the lock now names the new
> bytes`. This is the same boundary `package-members-v1` draws: a pinned member
> is not a running one. Routing is read through `agentstack delivery`.

## Package-aware delivery

Three concepts. This vocabulary governs the design and the user-facing copy.

| Concept | Purpose | Exists today as |
|---|---|---|
| **Package** | A reusable, versioned composition of capabilities | The vendored pack rail — `pack.toml` (`crates/cli/src/provider/gitpack.rs`, `PackToml`: one optional server, skills, instructions), the install ledger, digest pins, and `lock --upgrade` |
| **Toolset** | The subset a particular project or task selects | Profiles in the manifest (`crates/core/src/manifest/model.rs`, `Profile`: servers + skills) |
| **Lease** | The temporary runtime activation of one toolset over an MCP connection | The in-memory MCP lease |

The pack rail is **not new work**: versioned git-hosted packages with a ledger,
digest-pinned members, upgrade-with-member-diff, and source policy enforced
before fetch all ship today. What this lane adds is unification — connecting
that rail to the central library, the delivery planner, and the panel.

### v1 scope, stated precisely

Package-aware v1 covers **servers, skills, and instructions** — exactly what
`pack.toml` carries. A generalized package carrying **hooks or extensions is
deferred by name**: it is new schema work, and those are full-ceremony
executable kinds regardless, so nothing about their consent would compress.

`packages = [...]` in a toolset is likewise **new schema work**, including the
semantics of an instruction member selected through a toolset (today a `Profile`
selects servers and skills and nothing else). It belongs to Workstream 5:

```toml
[toolsets.backend]
packages = ["rust-backend"]
skills   = ["project-specific-review"]
servers  = ["project-database"]
```

The lock expands that into the exact package version and revision, the exact
member list, per-member content digests, and per-member provenance.

### Copy versus live reference, settled

The unsafe design is resolving a package from whatever is currently in the
library **at runtime**. A compact central package *reference* in the manifest is
exactly as safe as vendored copying **iff the lock pins the expanded member
set** — same bytes, same consent, better UX.

So the model is hybrid: keep the mature vendored mechanism, and add the compact
reference as a UX layer that **compiles into exact lock entries**. This requires
the library to gain a first-class **package index**; today `Library`
(`crates/cli/src/library.rs`) indexes skills, servers, extensions and hooks, and
has no notion of a pack.

### Boundary, not bodies

Activating a package makes its capability *boundary* available, not its
contents: member skills' names and descriptions become discoverable, and only
the selected servers' tools are exposed. Skill bodies still load one at a time,
on demand, digest-verified. A server should start — or connect — only when one
of its tools is first called (lazy server start, Workstream 5).

Eagerly injecting twenty skill bodies because a package was selected would
recreate the context-bloat problem this lane exists to solve, under a new name.

## Failure semantics

Three behaviours must be *defined and witnessed* before the flip. Each is a
place where "dynamic" could silently widen what is reachable.

**1 · Toolset fencing.** With several toolsets declared and no lease open, the
gateway must never expose the implicit union of everything declared. The rule:
**no lease → control-plane tools only.** Capability exposure requires an
explicit selection — a lease naming the toolset — and the union is never served
implicitly. No "active toolset" designation is introduced to soften this: the
`Profile` schema has no such field today, adding one is not part of this
decision, and a default-exposed toolset would be a second, quieter way for
capabilities to reach a harness with no explicit selection behind them.

**2 · Mixed-lane atomicity.** The consent unit is the *composition*. Drift in
any member — dynamic or rendered — marks the project **Changed**, and Changed
blocks new leases and new loads at the choke points we control, while status
names the drifted member. Files already rendered stay on disk and **status says
so**: a file already written cannot be un-served by freezing MCP, and claiming
otherwise would be the exact dishonesty invariant 8 forbids.

**3 · Gateway unavailable.** Fail closed. The harness gets no tools; `status`
and `doctor` explain the outage in one sentence and name the one recovery
command. AgentStack **never silently switches to a writing lane** — a static
fallback render is always an explicit user action.

## Lease lifecycle

A lease today lives in the MCP subprocess's memory and is invisible to every
other surface: no snapshot, doctor view, or panel can see "a lease is open on
toolset X with four skills loaded".

The contract: a **machine-level runtime lease registry** carrying the lease
instance ID plus PID and process start-time validation, queried through an
authoritative CLI read path, and exposed to the panel as a new
`lease-status-v1` ui-contract feature (named in `crates/cli/src/ui_contract.rs`,
which stays the single source of truth for advertised feature strings).

Explicitly **not** a JSON state file treated as current truth. PID plus process
start-time validation is what distinguishes a live lease from a stale record
after a crash or a PID reuse; a file read as truth invites exactly the
stale-state bug this registry exists to prevent.

## t3code surfaces

All reads over existing contracts plus the new registry. The panel gains **no
new authority** — the boundary in
[`ui-control-plane.md`](../archive/design/ui-control-plane.md) (archived; cited
normatively) is unchanged, and no per-item consent answer is collected in the
panel.

- Packages installed and available, with version and update status.
- Effective members after this project's overrides.
- Active toolset and the delivery routing that produced it.
- Lease state (`lease-status-v1`).
- Skills loaded this session.
- Servers available versus actually started.
- Trust and lock failures, each with a direct repair action.

The panel gets *simpler* under this default: drift review, render state, and
gitignore management leave the everyday path for dynamic-lane projects, leaving
toolset, lease, and the yes queue as the daily surface.

## Honesty rules (binding on every surface)

These are copy rules with the same force as the invariants below. They exist
because each false-friendly phrasing was written before it was caught.

- **"0 project artifacts for gateway-delivered capabilities."** Never a bare
  "0 files": the project still holds a manifest, a lock, and — whenever
  instructions are used — a managed region in an instruction file.
- **A separate `rendered lane:` line** naming what was actually written and
  where, on any status or result that also reports the dynamic lane. Never one
  blended sentence.
- **"Every brokered MCP call recorded."** Never "every call recorded".
  AgentStack records what was asked of a server; it cannot observe that server's
  internal side effects. The recorder is evidence of the request, not of
  everything the server then did.
- **Recording is not prevention** and **an allowed destination can still
  exfiltrate** — unchanged from [`../ENFORCEMENT.md`](../ENFORCEMENT.md), and
  the lease column must not imply otherwise.

## Workstreams

Five, with the acceptance criterion each is finished against. Order matters
where stated; **W4 lands last** because it contains the flip.

### W1 — The yes on the lease path

The consent UX for the dynamic lane: a refused lease or load emits an event that
the CLI and the panel render as a yes-card prompt, using the existing card.

*Acceptance:* a refusal names what was refused and the one command that fixes
it; the card rendered from a refusal discloses no less than the card rendered
from `agentstack trust`; and a witness asserts that **no MCP-invocable consent
path exists** — the agent can relay the refusal and can never answer it.

### W2 — Trust checked at dispatch — **security release blocker**

Digest-authoritative trust comparison on every upstream dispatch, emptying the
**upstream capability surface** on revoke or drift while control-plane tools
stay available. A generation token is a cache only; uncertainty recomputes and
fails closed.

*Acceptance:* three witnessed cases, each stopping the **next** upstream call on
an already-established connection —

1. **Trust revoked** mid-connection.
2. **Out-of-band manifest modification** — an edit no AgentStack command made,
   so nothing in-process could have been notified.
3. **Lock replacement** — the lock file swapped wholesale, which is what a
   `git pull` or a branch switch actually does.

Plus: after each, control-plane tools are still reachable (the user can
diagnose and recover); and the dispatch path stays single — no second authority
constructor, no second transport. Reviewed line by line — it sits on the
authority path.

### W3 — Update semantics

Immutable pinned-byte serving from the content store, the announce-only
`lib sync` line, `status`'s update offer, and the explicit upgrade flow
including the mixed-lane transactional report.

*Acceptance:* a `lib sync` that pulls changed bytes changes **no active bytes,
no lease, no trust state, and no rendered file** in any project — while making
update availability **observable through `status`**; a project keeps serving its
pinned bytes afterwards; upgrading
shows the aggregate package diff then per-member diffs; a mixed-lane upgrade
either updates both the lock and the rendered region or neither, and reports the
two lanes on separate lines with no "live via gateway" claim over an
instruction.

### W4 — Planner, registry, and the flip — **last**

The runtime lease registry and `lease-status-v1`; planner routing wired into
`init` and onboarding (with the Automatic / Render locally override — amended
2026-08-02); the `ENFORCEMENT.md` lease column; and the default flip itself.

*Acceptance:* a lease is externally visible with honest liveness (PID plus start
time), and a stale record never reads as live; `init` states the routing per
harness in plain language; `ENFORCEMENT.md` carries an honest lease column
stating what that path *does* enforce (it is the strongest column — every
capability call is brokered, policy-checked and recorded — and it still says
plainly what recording is not); and the flip lands only with every precondition
below satisfied. Two of those preconditions are witnessed here, because they are
this workstream's own semantics:

- **Toolset fencing (precondition 3).** A project with several toolsets declared
  and no lease open serves **control-plane tools only**; opening a lease exposes
  exactly the named toolset's members and nothing more.
- **Gateway unavailable (precondition 6).** With the gateway unreachable the
  harness receives **no tools**; `status` and `doctor` each produce the
  one-sentence outage explanation naming the one recovery command; and **no file
  is written automatically** — no silent fallback into the rendered lane.

### W5 — The package layer

The library package index; the toolset `packages = [...]` schema including
instruction-member semantics; per-member project overrides; lazy server start;
and the panel's package surfaces. **Hooks and extensions in packages are
deferred by name.**

*Acceptance:* a package selected in a toolset expands in the lock to exact
members with per-member digests and provenance; the boundary is exposed without
loading bodies; a server starts on first tool use, not on activation; and a
per-member override is visible as an *effective member set* rather than silently
diverging from the package.

## The preconditions for the flip (amended 2026-08-02)

The default flips only when all remaining seven hold; the flip lands with W4,
at arc-end (amended 2026-08-02).

1. Trust invalidation on every live dispatch — digest-authoritative, generation
   token as cache only (W2).
2. Immutable pinned-library behaviour — sync announces updates, never interrupts
   a project (W3).
3. Deterministic toolset fencing (W4, semantics defined above).
4. Externally visible lease ownership and lifecycle — a runtime registry, not a
   state file read as truth (W4).
5. Defined mixed-lane failure semantics, witnessed (W2/W3).
6. Defined gateway-unavailable recovery, witnessed (W4).
7. The advanced delivery override — Render locally (W4; amended 2026-08-02).
8. ~~The §1.6 activation study, run on v0.18.0-rc.2 as pinned.~~ **Removed
   2026-08-02:** the study runs when v3's bar is met and no longer gates the
   flip.

### Verified 2026-08-03 — the flip landed

Each precondition, the code that satisfies it, and the witness that holds it.

| # | Satisfied by | Witness |
|---|---|---|
| 1 | `TrustAnchor` re-verified from disk on every upstream dispatch and every `tools/list`; no generation-token cache exists at all, so nothing unauthoritative can shortcut it (`crates/cli/src/trust_anchor.rs`, `gateway.rs`) | `crates/cli/tests/trust_at_dispatch.rs` (7) — revoke, out-of-band manifest edit, and wholesale lock replacement each stop the **next** call on a live connection; control-plane tools survive |
| 2 | `Store::pinned_content` serves skill bodies from the content-addressed snapshot, never the live library; `use`/`add` materialize from the same snapshot; `lib sync` announces through `status` and writes nothing into any project | `crates/cli/tests/lib_sync_does_not_disturb_projects.rs` (6) — a sync leaves every project byte-identical including symlink targets, and a project keeps serving its pinned bytes |
| 3 | The unleased fence: a project declaring any toolset gets `Gateway::empty()` until a lease names one (`crates/cli/src/mcp_server.rs`, `AutoProject::activate`). Found **open** during the registry half and closed there | `crates/cli/tests/lease_registry.rs` — `no_lease_means_control_plane_tools_only_even_with_several_toolsets_declared` and `opening_a_lease_exposes_exactly_that_toolset` |
| 4 | `crates/cli/src/lease_registry.rs` — a record is persisted per lease, and **liveness is derived at read time** from the recorded PID *and* that process's start token; never read from the file as truth. `agentstack lease status [--json]` is the authoritative read (`lease-status-v1`) | `crates/cli/tests/lease_registry.rs` — an open lease visible to another surface; a stale record never reading live, proven by a dead PID *and* by a live PID whose start time disagrees (simulated reuse) |
| 5 | Drift in any member marks the project Changed and blocks new leases and loads at the existing choke points; a mixed-lane upgrade updates the lock **and** the rendered region or neither, inside one rollback envelope | `crates/cli/tests/upgrade_lanes.rs` (4) — the all-or-nothing transaction proven by a real failure injection, with separate lane lines and no "gateway" claim over an instruction |
| 6 | Gateway-unavailable detection in `crates/cli/src/commands/connect.rs` (`gateway_outages` / `command_unreachable`), with one sentence stem shared by `status` and `doctor`, and no writing path anywhere on it | `crates/cli/tests/lease_registry.rs` — `an_unavailable_gateway_yields_no_tools_and_writes_no_file`, which also asserts the project tree is byte-for-byte identical afterwards |
| 7 | **Render locally** — `[delivery] render_locally`, per project and per harness (`crates/core/src/manifest/model.rs::Delivery`), set by `agentstack delivery render-locally [--harness <id>] [--off] --write` and offered behind the wizard's "more control" path | `crates/cli/tests/delivery_planner.rs` — `render_locally_writes_files_where_the_lease_would_have_worked`, both scopes, including a per-harness entry overriding a project-wide one in each direction |

**What "default" means here, precisely.** The planner
(`crates/cli/src/delivery.rs`) routes skills and MCP servers on an MCP-capable
harness to the dynamic lane with no override present, and the onboarding wizard's
default answer is **Automatic** — it states the routing, offers the one bridge
registration the live lane needs, and renders nothing. `agentstack apply` is
unchanged and still renders everything it is asked to: it is the rendered lane's
command, and running it is the explicit user action §Failure semantics 3 requires
a fallback render to be. Static rendering was not removed anywhere.

## Invariant check

Against `CLAUDE.md`'s non-negotiable invariants, one line each.

- **No new unsafe code** — nothing in this lane touches the CLI's `sys.rs`
  boundary.
- **Policy only narrows** — lease calls are policy-checked at the existing choke
  point; the machine ceiling is unchanged.
- **Untrusted content is inert** — strengthened: the gateway refuses to *serve*,
  not only to write, and W2 extends that refusal to live connections.
- **Pinned byte changes re-gate** — extended to freezing new serving and raising
  the diff card; no cache or partial-trust path is introduced, because the
  generation token is never authoritative.
- **Secrets never serialize** — unchanged; leases resolve `${REF}` in memory and
  fail closed when unresolved.
- **Authority and dispatch stay single-path** — W2 tightens the existing
  dispatch; it adds no second grant constructor and no second transport.
- **All repository content is hostile input** — unchanged; package members and
  their metadata are bounded and sanitized on the existing intake path.
- **Claims match enforcement** — the honesty rules above are binding copy rules,
  and the lease column documents what that path really does.
- **Hooks and extensions are executable kinds** — they never enter the dynamic
  lane, and no compressed-consent path covers them, in a package or out of one.

## The risk, stated plainly

The consent-surprise risk moves from *"files appeared"* to *"calls happened"*.
It is mitigated by the flight recorder (every brokered MCP call recorded) and by
the yes queue making pending consent visible — and it is the reason W2 is a
release blocker rather than a hardening item. The thing to protect from here:
"dynamic" must never become another mechanism a user has to reason about. One
automatic setup, transparent per-harness coverage, and pins that stay put until
an explicit upgrade.
