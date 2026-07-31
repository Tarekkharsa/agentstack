# CLAUDE.md — AgentStack

## Product direction

AgentStack is the **vendor-neutral environment manager for AI coding tools**.
Its promise is:

> **Define your agent setup once. Use it across every coding CLI.**

Users come for portability, easy setup, named toolsets, reversible activation,
and reliable diagnosis. Security makes those benefits dependable; it is not the
opening lesson or a separate product.

The product has two interfaces:

- **The AgentStack CLI is the primary surface and the launch channel**, as well
  as the source of authority and automation contract. It owns all validation,
  writes, consent checks, and enforcement — and it is the thing a stranger can
  obtain and run today, which is what makes it the surface the product is judged
  on. Setup, toolset selection, status, and recovery must be complete and
  legible here first.
- **t3code is the optional graphical companion.** It calls stable read APIs and
  a closed set of fixed actions; the frontend is never an enforcement boundary.
  It is where some users graduate, not how they arrive. Revisit this when t3code
  is publicly obtainable — it is a private fork with no download today, which is
  precisely why it stopped being called the launch channel.

The old embedded AgentStack dashboard was removed. Do not recreate a second UI.
Improve t3code or the CLI/API that supports it.

Read in this order:

1. `STRATEGY.md` — the operative v2 strategy: north-star goal, the design law
   ("automate everything except the yes"), and the phased gates. v1 is archived
   under `docs/archive/`; never take direction from the archive.
2. `TODO.md` — the only ordered work queue.
3. `docs/ARCHITECTURE.md` — system boundaries.
4. `docs/ENFORCEMENT.md` — exactly what each mode does and does not enforce.

Design documents explain active technical contracts. They are not additional
roadmaps. `CHANGELOG.md` is the historical record.
`docs/design/README.md` indexes them: which document answers which question,
and which are closed records rather than live contracts. Read it before opening
one, not after.

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
- trust → **review this project** when the gate actually appears; strategy v2
  names the consent moment itself **the yes**
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

Strategy v2 classification: **hooks are an executable capability kind alongside
extensions** — they run commands in or around the harness at user permission,
so the full consent ceremony always applies; no compressed-consent path may
ever cover them.

`trust` and `policy` remain small review boundaries. Any new dependency requires
maintainer approval. The approved Boa dependency stays isolated in `workflow`;
its module loading and other ambient capabilities must remain explicitly
disabled or brokered.

## Context and test discipline

The two scarcest resources in a session are context tokens and local test
minutes. Two hard rules bound them:

- **Never read compiled docs into context.** Every `.html` under `docs/`
  (root pages, `howto/`, `tutorial/`, `panel/`, `design/`) is build output of
  a sibling `.md` source or of `tools/make-docs-pages.py` itself — up to
  128&nbsp;KB per page of pure noise for a model. Read and edit the `.md`,
  regenerate with `python3 tools/make-docs-pages.py`, verify with
  `python3 tools/check-docs-site.py`. `.claude/settings.json` denies the Read
  tool on these paths and on `target/`; for the rare structural check of
  generated output, use `grep` through Bash. `docs/theme/` is source and
  stays readable. `docs/archive/` is history — open it only when researching
  lineage, never for direction.
- **Never run the full test suite locally.** The workspace has 60+ test
  binaries; a bare `cargo test` at the root (or `--workspace`) costs minutes
  and belongs to CI. The loop is: `cargo check -p <crate>` while iterating;
  `cargo test -p <crate>` — or `cargo test -p agentstack-cli --test <name>`
  for a single binary — for the crates the change can actually break;
  `cargo fmt --check` before handoff. Run exactly the tests your change can
  break, nothing more.

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
