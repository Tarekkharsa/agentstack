# AgentStack product strategy

*Current as of agentstack 0.18.0, 2026-08-12. Rebuilt from the repository:
every claim below describes what the code does today, not what was planned.*

> **Relationship to the other documents:** [`TODO.md`](TODO.md) is the only
> ordered work queue. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and
> [`docs/ENFORCEMENT.md`](docs/ENFORCEMENT.md) describe what the code does and
> enforces; where any document disagrees with `ENFORCEMENT.md`, that one wins.
> This file is the map: what the product is for, what it will not become, and
> what has to be true before the next step.

## The goal

> **Any capability, from anywhere, live in every agent you run — seconds after
> one deliberate yes, and never any other way.**

Identity in one line: **feels like a filesystem, thinks like a vault.**

**From anywhere** means origin stops mattering because review does — a file
dropped in, a repo cloned, a teammate's bundle, an external registry.
**Every agent you run** is the portability promise a single-runtime framework
structurally cannot make. **One deliberate yes** is consent compressed into a
single glanceable, content-bound moment — compressed, never removed.

## The design law

> **Automate everything except the yes.**

Pinning, locking, staging, rendering, drift repair, recovery: all of it is the
system's job, done silently and correctly. The manifest and lock are
system-maintained — written by the machine in the common path, read by humans,
reviewed in pull requests — and the manifest remains the source of truth. The
one thing never automated, inferred, or defaulted is consent to new content.

## What the product is today

All of the following ships in 0.18.0:

- **Thirteen adapters.** One manifest renders to every supported CLI's native
  format; a nightly conformance job proves the real tools still accept it.
- **Routed delivery, not chosen.** Skills and MCP servers are served live
  through the gateway to MCP-capable harnesses, so the project stays clean.
  Instructions, settings, hooks, and extensions are written into native files,
  because no live channel a harness is known to consume carries them
  correctly — as is every capability on a tool without MCP. The system picks
  the lane per capability and per tool; the user does not.
- **Consent is content-bound.** An untrusted project's servers, skills,
  instructions, hooks, and extensions do not render. Editing the manifest or
  lock re-gates. Executable kinds always get the full ceremony.
- **A policy ceiling that only narrows.** The machine's policy intersects the
  project's; a repository can never loosen it.
- **A protected `run` by default**, with `--unprotected` as the explicit
  opt-out, plus Docker `--sandbox` and no-direct-route `--lockdown` with
  compiled egress and filesystem policy.
- **Toolsets and leases.** One selection concept — project selection, lease
  unit, and workflow role are the same noun — with a lease registry behind the
  MCP control plane.
- **Images.** `agentstack x image` composes one toolset's pinned members into a
  self-run Docker image.
- **Evidence and recovery.** Per-run reports, a call audit log, and byte-exact
  restore of managed writes.

## Who it is for

Developers personally running two or more coding CLIs. **Beachhead pair:
Claude Code + Codex** — first-run material demonstrates this pair, and it has
to be flawless. Someone using one CLI with a small hand-managed setup does not
need this yet; it earns its place when the same setup is repeated across tools,
projects, machines, or teammates.

The repository is open to the community now: the installer, the documentation
site, and the contribution and governance files are public, and issues are
welcome. What has not happened yet is the launch push.

## What never changes

The floor under the plan. None of this is negotiable; the plan relocates
cost, never relaxes guarantees.

- Untrusted repository content is inert. The funnel stages; it never activates.
- All repository content is hostile input: parsed defensively, bounded, never
  interpolated into shell commands.
- No new unsafe code.
- Consent is content-bound. A byte change re-gates — the presentation of
  re-gating improves; the fact of it does not.
- Policy only narrows. Machine ceiling always wins.
- Secrets never serialize. `${REF}` resolves in memory; unresolved fails closed.
- Single authority and dispatch paths, with witnesses.
- Claims match enforcement. Every convenience states honestly what it does
  and does not enforce.
- **Executable kinds — native extensions and hooks — keep the full ceremony.**
  Never a compressed review, in a package or out of one.
- Progressive disclosure must never become progressive enforcement.

## Also binding

- **The promise:** *Define your agent setup once. Use it across every coding
  CLI.* Cross-vendor portability is the product; trust, policy, and evidence
  make it dependable.
- **Surfaces:** the CLI is the primary surface and the sole authority. A
  graphical companion works over the same fixed, digest-bound action contract
  — never a second authority, never a second enforcement boundary.
- **Composition:** manifest, toolset, and the library. The linked library
  sources are the reuse path across projects; the funnel feeds it, it does not
  replace it.
- **Engineering:** extend existing seams; never reimplement working trust,
  policy, gateway, runtime, recording, import, render, or restore paths.
  `trust` and `policy` stay small review boundaries.

## What is deliberately not built

No agent-building framework or general coding agent. No hosted multi-tenant
runner. No Cloudflare-specific product — a workers deploy *target* among
several, on the user's own account, is not that. No public marketplace before
local reuse earns one. No enterprise administration suite. No background-jobs
platform. No separate repositories for components without independent
adoption. No second embedded dashboard, and no second UI of any kind.

## Competitive watch

*As of 2026-08.* eve (Vercel) is not a category competitor: it builds and hosts
one new agent rather than managing the environment of existing CLIs, and it has
no content trust gate. Its ecosystem is machine-readable supply for governed
intake.

No tripwire has fired. Tripwire 3 is trending: the de-facto skill distributor
today is **vercel-labs/skills** (`npx skills`, 75+ target agents including both
beachhead CLIs), which eve's own docs recommend. In the same family, major CLIs
are shipping config-import (Codex `/import` migrates Cursor and Claude Code
configs).

Each tripwire warrants a strategy revisit, never an automatic build:

1. eve (or a sibling project) imports existing CLI setups.
2. eve renders or exports configuration to other harnesses.
3. eve's registry or vercel-labs/skills becomes de-facto skill distribution
   and users ask us to consume it.
4. A content-bound trust gate ships in that ecosystem.

## Open design question

**Scoped-MITM credential brokering** — it extends what the vault protects, not
how the yes works. It keeps its own design-doc lane and is not part of this
plan.

## What happens next

1. **Cut stable v0.18.0.** The documented install still pins a release
   candidate because `releases/latest` never points at a pre-release; a stable
   tag retires that pin and the warning that goes with it.
2. **Run the activation study.** The kit is
   [`docs/design/activation-study.md`](docs/design/activation-study.md); it
   waits on its own §0 re-pin to the current release candidate and refuses to
   be run until that is cleared. Its five thresholds and three-blocker rule are
   unchanged: five participants using 2+ agent CLIs; ≥4/5 finish unaided;
   median install→clean `doctor` under five minutes; ≥4/5 say "one setup across
   my CLIs"; 5/5 need no advanced concepts; ≥4/5 understand every block. The
   three blockers are the study's *output*, so they cannot be worked before it
   runs.
3. **Then the launch push.** Show HN as the primary moment, same-day posts in
   the Claude Code and Codex communities, and a slow-burn presence in the
   skills ecosystems positioned as the governed way to use what those
   registries distribute. The announcement leads with portability, with trust
   as the second beat, and never hitches to a competitor's name.

The documentation site and release candidates ride ahead of that moment — they
ship as the product ships. Only the launch push waits on the study.

## How this document is used

`TODO.md` is the queue; this is the map. Deviations edit `TODO.md`, never this
file. This document reopens at a named tripwire above, or when the code it
describes stops matching it — and the code is what wins.
