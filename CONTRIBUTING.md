# Contributing to agentstack

Thanks for looking under the hood. agentstack is a solo-maintained, pre-1.0
security tool; contributions are welcome, and the bar that matters most is
the one the code already holds itself to: **claims match enforcement, and
security claims ship with a test that witnesses them.**

## Orientation

Read in this order:

1. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — the layer model and crate
   boundaries.
2. [`docs/ENFORCEMENT.md`](docs/ENFORCEMENT.md) — what is actually enforced,
   per mode. When any doc disagrees with it, it wins.
3. [`STRATEGY.md`](STRATEGY.md) and [`TODO.md`](TODO.md) — the phase gates
   and the current work queue. Please don't open PRs for later-phase work;
   the ordering is deliberate.

## Build and test

```bash
cargo build --workspace
cargo nextest run --workspace     # or: cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

All three checks must pass before a PR is ready. The Docker sidecar tests
(`crates/egress/tests/sidecar_image.rs`) are `#[ignore]`d locally; CI's
sandbox job runs them with `--include-ignored`.

## Ground rules (not preferences)

These are security requirements. PRs that relax them will be declined even
when the change "works":

- **No `unsafe`.** Every crate carries `#![forbid(unsafe_code)]`; the one
  exception is `crates/cli/src/sys.rs`, which concentrates the workspace's
  entire unsafe surface (a handful of libc process-management calls) behind
  a single `#[allow]`. Don't add unsafe anywhere else.
- **Policy can only narrow.** The effective policy is the intersection of
  bundle policy and machine policy — never more permissive than the machine.
  The proptest invariants in `crates/policy` witness this per dimension;
  they are never deleted or weakened.
- **Untrusted means inert.** Until a bundle's digest is trusted, no server
  spawns, no skill enters context, no secret resolves. No dev-mode
  exceptions.
- **Any pinned byte change re-gates trust.** No caching or fast path may
  weaken the content binding (`crates/trust` has the byte-flip proptest).
- **Secrets never serialize.** `${REF}` placeholders resolve only at
  runtime, in memory; unresolvable secrets fail closed.
- **Bundle content is hostile input.** Manifests, lockfiles, skills, and
  server definitions come from unreviewed repos: parse defensively, bound
  sizes, never interpolate into shell commands, and don't `unwrap`/`expect`
  on anything derived from them.
- **Dependencies are restricted.** `trust` and `policy` have a fixed,
  minimal dependency list; adding any new dependency anywhere in the
  workspace needs maintainer approval first — propose it in the PR
  description, don't just add it.
- **Crate edges are fixed.** The permitted internal dependency graph is in
  `docs/ARCHITECTURE.md`; anything not listed is forbidden.

## What a good PR looks like

- **Small and single-purpose.** One capability or fix per PR.
- **A witness per security claim.** If the change touches trust granting,
  policy intersection, digest computation, secret resolution, or an
  enforcement path, it ships with a test proving the claim — and the PR
  description says so explicitly, because those diffs get line-by-line
  review.
- **Docs move with claims.** If behavior changes what a mode enforces,
  update `docs/ENFORCEMENT.md` in the same PR. Never let README or site copy
  claim more than the matrix backs.
- **No drive-by test suites.** One focused test per new behavior is the
  house style; mechanical plumbing often needs none.

## Easiest first contribution

Adding a CLI adapter is one data-driven YAML descriptor — copy
`crates/adapters/descriptors/codex.yaml`, check it with
`agentstack adapters validate my-adapter.yaml`, and drop it into
`~/.agentstack/adapters/` to test without a rebuild.

## Reporting a vulnerability

See [`SECURITY.md`](SECURITY.md) — please use private reporting rather than
a public issue.
