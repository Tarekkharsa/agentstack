# CLAUDE.md — AgentStack

## Where the product stands

The maintainer reset the strategy documents on 2026-08-02 and adopted a new
strategy the same day. The operative direction is v3:

- [`STRATEGY.md`](STRATEGY.md) (v3, adopted 2026-08-02) is the operative
  product strategy and the only strategy reference. It carries the goal, the
  shape, the invariants, the bar, and the named revisit triggers.
- [`TODO.md`](TODO.md) is the only ordered work queue, re-seeded from v3's
  plan. It is the sole sequencing authority; deviations edit the queue, never
  the strategy.
- `docs/archive/` and git history hold everything older, including v2.
  History, never direction — do not resurrect plans or roadmaps from there.
- The live design contracts are
  [`docs/design/automatic-delivery.md`](docs/design/automatic-delivery.md)
  (the delivery contract) and
  [`docs/design/activation-study.md`](docs/design/activation-study.md) (the
  instrument for the bar-met moment);
  [`docs/design/README.md`](docs/design/README.md) indexes them.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and
  [`docs/ENFORCEMENT.md`](docs/ENFORCEMENT.md) describe the shipped system
  and remain authoritative for what the code does and enforces.

Do not start capability lanes outside v3's shape. The shape is the committed
set; nothing beyond it starts before the bar is met. Propose, discuss, and
let the maintainer adopt direction explicitly.

Two standing product constraints hold: the AgentStack CLI is
the primary surface and source of authority (t3code is an optional graphical
companion calling stable read APIs and fixed actions, never an enforcement
boundary), and the old embedded dashboard stays removed — never recreate a
second UI.

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

Standing classification: **hooks are an executable capability kind alongside
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
  `python3 tools/check-docs-site.py`. For the rare structural check of
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
- **Compile-verify with `cargo check --workspace --all-targets` before
  handoff.** It is fast, and it is the only cheap check that sees test
  targets. `cargo check` skips `cfg(test)`, and `cargo test --test <name>`
  builds one binary — so a change to a widely-constructed type compiles and
  passes everything a focused loop runs while leaving other test targets
  unbuildable. This is a structural blind spot in the focused loop above, not
  a lapse of care: it broke the build twice in one session on the same change
  before being caught by an unrelated `--all-targets` run.

## Working rules

- The strategy is being redefined; do not start new capability lanes. Propose
  and discuss; only maintainer-adopted direction authorizes work.
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
