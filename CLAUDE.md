# CLAUDE.md — AgentStack

Shipped Rust workspace. Extend existing seams; do not reimplement working
trust, policy, gateway, runtime, recording, import, render, or restore paths.
Prefer clear, boring, idiomatic Rust; explain non-obvious ownership/lifetime/
trait/error choices briefly.

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

Authorities: `STRATEGY.md` (strategy), `TODO.md` (the only work queue),
`docs/ARCHITECTURE.md` + `docs/ENFORCEMENT.md` (what the code does and
enforces). `docs/archive/` is history, never direction. The CLI is the primary
surface and source of authority; t3code is an optional companion calling stable
read APIs and fixed actions, never an enforcement boundary. Never recreate a
second UI.

## Invariants

The non-negotiable invariants live in `STRATEGY.md` ("What never changes")
and `docs/ENFORCEMENT.md`; every change must preserve them. Hooks always get
the full consent ceremony; `trust` and `policy` stay small review boundaries;
new dependencies need maintainer approval; Boa stays isolated in `workflow`.

## Hard rules — context and tests

- Never run the full test suite locally. Loop: `cargo check -p <crate>` while
  iterating; `cargo test -p <crate>` (or `--test <name>`) only for crates the
  change can break; full suite belongs to CI.
- Before handoff: `cargo check --workspace --all-targets` (the only cheap
  check that sees all test targets), `cargo fmt --check`, relevant clippy.

## Hard rules — build cache

- Building in `.claude/worktrees/*`: export
  `CARGO_TARGET_DIR="$HOME/.cache/agentstack-target"` first, so every worktree
  shares one incremental cache instead of recompiling the workspace per tree.
- Debug builds by default; `--release` only when CI parity demands it.
- Removing a worktree: delete its local `target/` with it.
