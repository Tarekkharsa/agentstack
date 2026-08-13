## What / why

<!-- What changed, and why. Link an issue if there is one. -->

## Checklist

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] Relevant tests pass (`cargo nextest run -p <crate>` or a filtered run — not the full workspace suite for every iteration)
- [ ] This change touches trust granting, policy intersection, secret resolution, or digest computation → flagged for line-by-line review
- [ ] **Enforcement pairing** — if this changes enforcement behaviour in `crates/trust`, `crates/policy`, or `crates/egress`, `docs/ENFORCEMENT.md` changes in this same PR. If it deliberately does not, put the waiver on its own line in this PR body: `ENFORCEMENT-WAIVER: <one-line reason>` (a reason is required; a bare marker does not satisfy the gate)
