# CLAUDE.md — AgentStack

## Product direction

AgentStack is the **vendor-neutral environment manager for AI coding tools**.
Its promise is:

> **Define your agent setup once. Use it across every coding CLI.**

Users come for portability, easy setup, named toolsets, reversible activation,
and reliable diagnosis. Security makes those benefits dependable; it is not the
opening lesson or a separate product.

The product has two interfaces:

- **t3code is the primary graphical experience and launch channel.** Build
  setup, toolset selection, status, recovery, and contextual safety guidance
  there.
- **The AgentStack CLI is the source of authority and automation contract.**
  It owns all validation, writes, consent checks, and enforcement. t3code calls
  stable read APIs and a closed set of fixed actions; the frontend is never an
  enforcement boundary.

The old embedded AgentStack dashboard was removed. Do not recreate a second UI.
Improve t3code or the CLI/API that supports it.

Read in this order:

1. `STRATEGY.md` — product vision, progressive-disclosure rules, outcome gates.
2. `TODO.md` — the only ordered work queue.
3. `docs/ARCHITECTURE.md` — system boundaries.
4. `docs/ENFORCEMENT.md` — exactly what each mode does and does not enforce.

Design documents explain active technical contracts. They are not additional
roadmaps. `CHANGELOG.md` is the historical record.

## Product experience rules

The beginner experience exposes four ideas:

- **Setup** — detect and import the tools the user already has.
- **Toolset** — choose what the current project or task needs.
- **Status** — say whether it is ready and give one next action.
- **Undo** — make every material change recoverable.

Use progressive disclosure:

1. Show the useful outcome first.
2. Apply safe defaults silently when no decision is needed.
3. Explain a safety boundary only when it becomes relevant.
4. If an action is blocked, say what happened, why it matters, and the exact
   safe next step.
5. Put stronger modes and internal detail behind “More protection” or an
   equivalent advanced path.

Do not require Docker, policy authoring, gateway setup, trust terminology, or
workflow concepts to import and unify a normal local setup. Do not weaken an
invariant to make the journey shorter. Reduce the concepts and decisions the
user sees instead.

Prefer plain user language in UI and docs:

- profile → **toolset**
- doctor → **status/check setup**
- session → **use temporarily**
- trust → **review this project** when the gate actually appears
- policy/gateway/lockdown → **more protection**, with precise details available

## Existing system

This is a shipped Rust workspace, not a greenfield rewrite:

```text
crates/
  core/       manifest, lockfile, digests
  trust/      content-bound consent and signatures
  policy/     machine-first policy intersection
  adapters/   native config compilers for supported CLIs
  recorder/   call and run evidence
  runtime/    sandbox orchestration
  egress/     enforced network proxy
  executor/   policy-agnostic governed execution domain
  workflow/   self-contained Boa workflow engine
  cli/        binary, orchestration, JSON/action APIs
```

Extend the existing seams. Do not reimplement working trust, policy, gateway,
runtime, recording, import, render, or restore paths.

The maintainer is an experienced TypeScript developer learning Rust. Prefer
clear, boring, idiomatic Rust. Briefly explain non-obvious ownership, lifetime,
trait, or error-handling choices in code comments or the handoff.

## Non-negotiable invariants

1. **No new unsafe code.** Every crate forbids unsafe code except the CLI's
   existing, concentrated `src/sys.rs` process-management boundary.
2. **Policy only narrows.** Effective project policy is always a subset of the
   machine ceiling.
3. **Untrusted repository content is inert.** It cannot spawn/contact servers,
   enter agent context, or resolve secrets before the trust gate succeeds.
4. **Pinned byte changes re-gate.** Never add a cache or partial-trust path that
   weakens content binding.
5. **Secrets never serialize.** Manifests/configuration contain `${REF}`;
   unresolved values fail closed.
6. **Authority and dispatch stay single-path.** Do not create a second grant
   constructor or a second upstream transport path.
7. **All repository content is hostile input.** Parse defensively, bound it,
   and never interpolate it into shell commands.
8. **Claims match enforcement.** Host advisory checks are not confinement;
   recording is not prevention; allowed destinations can still exfiltrate.

`trust` and `policy` remain small review boundaries. Any new dependency requires
maintainer approval. The approved Boa dependency stays isolated in `workflow`;
its module loading and other ambient capabilities must remain explicitly
disabled or brokered.

## Working rules

- Work only on the current gate in `TODO.md`; new capability lanes require user
  evidence and an explicit strategy change.
- For non-trivial work, state a short plan, then implement unless a missing
  choice would materially change the result.
- Move existing code when extracting a boundary. Acceptance is preservation of
  the single authority/dispatch paths and their witnesses, not a line-count
  target.
- Keep tests proportional. Security claims require focused witnesses; ordinary
  plumbing needs enough coverage to prevent regression.
- Flag changes to trust granting, policy intersection, digest computation,
  secret resolution, authority construction, or upstream dispatch for
  line-by-line review.
- Before handing off, run `cargo fmt --check`, focused tests for touched crates,
  and relevant clippy checks. The full workspace suite belongs to CI unless the
  change crosses workspace-wide contracts.
