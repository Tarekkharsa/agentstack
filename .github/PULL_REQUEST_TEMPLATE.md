## What / why

<!-- What changed, and why. Link an issue if there is one. -->

## Checklist

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] Relevant tests pass (`cargo nextest run -p <crate>` or a filtered run — not the full workspace suite for every iteration)
- [ ] This change touches trust granting, policy intersection, secret resolution, or digest computation → flagged for line-by-line review
