# Contributing to agentstack

Thanks for looking under the hood. Participation is governed by the
[`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md); support scope and maintainer roles
are in [`SUPPORT.md`](SUPPORT.md) and [`GOVERNANCE.md`](GOVERNANCE.md).
AgentStack is a solo-maintained, pre-1.0 cross-CLI environment manager for AI
coding tools. It keeps one portable
configuration usable across otherwise incompatible clients, with trust,
policy, locking, and evidence as its security foundation. Contributions are
welcome, and the bar that matters most is the one the code already holds
itself to: **product behavior stays understandable, claims match enforcement,
and security claims ship with a test that witnesses them.**

## Orientation

Read in this order:

1. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — the layer model and crate
   boundaries.
2. [`docs/ENFORCEMENT.md`](docs/ENFORCEMENT.md) — what is actually enforced,
   per mode. When any doc disagrees with it, it wins.
3. [`STRATEGY.md`](STRATEGY.md) and [`TODO.md`](TODO.md) — the product
   direction, outcome gates, and current work queue. Please don't open PRs for
   later-stage work; the ordering is deliberate.

## Build and test

**While you iterate, stay narrow.** The full workspace is slow, and CI owns it:

```bash
cargo check -p <crate>            # the loop you run constantly
cargo test -p <crate>             # or: --test <name> for a single test file
```

`-p` takes the *package* name, which is not the directory name — and
`crates/cli` is the odd one, since its package is the binary's name. The
mapping:

| Directory          | Package name            |
| ------------------ | ----------------------- |
| `crates/cli`       | `agentstack`            |
| `crates/adapters`  | `agentstack-adapters`   |
| `crates/core`      | `agentstack-core`       |
| `crates/egress`    | `agentstack-egress`     |
| `crates/executor`  | `agentstack-executor`   |
| `crates/mcp`       | `agentstack-mcp`        |
| `crates/policy`    | `agentstack-policy`     |
| `crates/recorder`  | `agentstack-recorder`   |
| `crates/runtime`   | `agentstack-runtime`    |
| `crates/trust`     | `agentstack-trust`      |
| `crates/workflow`  | `agentstack-workflow`   |

Widen to the workspace once, before you push:

```bash
cargo build --workspace
cargo nextest run --workspace     # or: cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

`cargo nextest` is the preferred runner — process-per-test isolation keeps the
env-var-mutating integration tests from interfering, and it is roughly 3x
faster. Install it with `cargo install cargo-nextest --locked`.

Every command above must pass before a PR is ready. The Docker sidecar tests
(`crates/egress/tests/sidecar_image.rs`) are `#[ignore]`d locally; CI's
sandbox job runs them with `--include-ignored`.

## Before you push

Every gate CI enforces, with the command that reproduces it locally. Run the
ones your change can plausibly break — you do not need the whole list for a
typo. They are transcribed from the three workflows that gate a pull request —
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) (build, lint, tests,
examples, MSRV, sandbox, docs, enforcement pairing, structure lint),
[`.github/workflows/supply-chain.yml`](.github/workflows/supply-chain.yml)
(`cargo deny`, on dependency-manifest changes), and
[`.github/workflows/docs.yml`](.github/workflows/docs.yml) (the docs checks
plus a headless-browser smoke, on `docs/`, `tools/`, or `CHANGELOG.md`
changes). Those files win if any of them disagrees with this list.
[`.github/workflows/conformance.yml`](.github/workflows/conformance.yml) is
nightly, not a PR gate.

**Always:**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo nextest run --workspace
```

**The feature-gated enforcement code**, which the two commands above do *not*
compile — so a regression there passes an otherwise green run:

```bash
cargo clippy -p agentstack-runtime --features docker --all-targets -- -D warnings
cargo clippy -p agentstack --features sandbox --all-targets -- -D warnings
```

**If you touched anything under `docs/`:**

```bash
python3 tools/check-docs-site.py          # links, fragments, sitemap
python3 tools/make-docs-pages.py          # regenerate the HTML pages
git diff --exit-code docs/                # must be clean: never hand-edit generated HTML
```

`docs.yml` additionally runs a headless-browser smoke (`tools/site-smoke.mjs`)
over the key pages; it needs Playwright and Chromium, so most contributors let
CI run it.

**If you touched a manifest kind or a policy dimension:**

```bash
python3 tools/check-structure.py
```

**If you changed `crates/workflow`'s dependencies**, re-bless the snapshot in
the same PR — the gate exists so a transitive-dependency change is a reviewed
event rather than a silent one:

```bash
cargo tree -p agentstack-workflow --edges normal --color never \
  | sed -E 's/^[^a-zA-Z]*//; s/ \(proc-macro\)//; s/ \(\*\)//; s# \(/.*\)##' \
  | grep -v '^agentstack-workflow ' | sort -u > crates/workflow/deps.snapshot
```

To check the snapshot without rewriting it — the exact form CI runs, which
fails on any drift:

```bash
cargo tree -p agentstack-workflow --edges normal --color never \
  | sed -E 's/^[^a-zA-Z]*//; s/ \(proc-macro\)//; s/ \(\*\)//; s# \(/.*\)##' \
  | grep -v '^agentstack-workflow ' | sort -u > /tmp/workflow-deps.txt
diff -u crates/workflow/deps.snapshot /tmp/workflow-deps.txt
```

**If you changed any `Cargo.toml`, `Cargo.lock`, or `deny.toml`:**

```bash
cargo deny check advisories
cargo deny check licenses sources bans
```

**MSRV** — a promise to anyone building on an older toolchain, and a different
number from the pinned development compiler in `rust-toolchain.toml`. It is
`rust-version` in the workspace `Cargo.toml` (currently **1.88**), so it moves
only deliberately. Needs `rustup toolchain install 1.88` once:

```bash
cargo +1.88 check --workspace --all-targets --locked
```

**The asserted example suite** (`examples/*/run-demo.sh`,
`examples/projects/*/assert.sh`) and the conformance self-test run against a
release build. Every one of these demos is CI-grade — isolated `HOME`,
PASS/FAIL checks, nonzero exit on failure — so CI runs the whole set, not a
sample. The script list is transcribed from `ci.yml` (the
`Malicious-repo demo` and `Example suite` steps); re-read it there when this
list looks stale:

```bash
cargo build --release
bash examples/sandbox/conformance-smoke.sh --self-test

export AGENTSTACK_BIN="$PWD/target/release/agentstack"
for script in \
  examples/malicious-repo-demo/run-demo.sh \
  examples/first-value-demo/run-demo.sh \
  examples/everyday-loop-demo/run-demo.sh \
  examples/guard-demo/run-demo.sh \
  examples/one-manifest-demo/run-demo.sh \
  examples/mcp-profile-lease/run-demo.sh \
  examples/projects/multi-cli-webapp/assert.sh \
  examples/projects/per-cli-instructions/assert.sh \
  examples/projects/policy-intersection/assert.sh \
  examples/projects/restricted-folders/assert.sh \
  examples/projects/skills-workout/assert.sh \
  examples/projects/locked-run/assert.sh \
  examples/projects/device-onboarding/assert.sh
do
  echo "--- $script"
  bash "$script" || { echo "FAILED: $script"; break; }
done
```

The Python MCP clients among them (lease demo, gateway probe, skills workout)
are stdlib-only, so a system `python3` is enough.

**Docker-backed enforcement**, run by CI's dedicated `sandbox` job — reproduce
locally only when you touched the sandbox, egress, or lockdown paths:

```bash
cargo test -p agentstack-egress --test sidecar_image -- --include-ignored --nocapture
cargo test -p agentstack --features sandbox --lib \
  --test sandbox_egress --test sandbox_cli_e2e --test sandbox_fs \
  --test sandbox_lockdown --test sandbox_gateway_e2e -- --nocapture
```

One gate has no local form: **enforcement pairing** runs on pull requests only,
because it diffs `base...head` and reads a waiver from the PR body. If you
change enforcement behaviour in `crates/trust`, `crates/policy` or
`crates/egress`, change [`docs/ENFORCEMENT.md`](docs/ENFORCEMENT.md) in the same
PR — or waive it explicitly. The waiver is a single line in the **pull-request
body** (a commit-message trailer on the branch also counts), spelled exactly:

```text
ENFORCEMENT-WAIVER: <one-line reason>
```

The reason is required: a bare marker with nothing after it is not a waiver and
does not satisfy the gate. Keeping it greppable is the point — every waiver
ever granted is findable with
`git log --grep 'ENFORCEMENT-WAIVER:'`. Test-only and comment-only changes in
those crates are already exempt, so a waiver should be rare. You can self-test
the checker with `python3 tools/check-enforcement-pairing.py --self-test`.

### T3 Code integration smoke

Changes to the T3 control-plane contracts can exercise the real bridge and all
four browser regressions (setup posture, CLI edit refusals, server startup
probe, and serial workflow roles) against a sibling T3 checkout:

```bash
cargo build --release
npm i playwright@1.54.0 --no-save --no-package-lock
npx playwright@1.54.0 install chromium
T3CODE_REPO=/path/to/t3code node tools/t3-integration-smoke.mjs
```

This check is optional and maintainer-only: it needs a local T3 Code checkout
that contributors outside the project may not have, so a PR is never held on it.

The runner uses the release AgentStack binary for T3's bridge E2E suite, then
starts T3 with isolated `HOME`, `AGENTSTACK_HOME`, and T3 data. Its deterministic
CLI fixture advertises and withholds individual feature contracts so the real
browser verifies both the intended UI and fail-closed behavior. It stops only
the exact process group it started and removes its temporary state.

## Ground rules (not preferences)

These are security requirements. PRs that relax them will be declined even
when the change "works":

- **No `unsafe`.** Ten of the eleven crates carry `#![forbid(unsafe_code)]`
  at their root: `adapters`, `core`, `egress`, `executor`, `mcp`, `policy`,
  `recorder`, `runtime`, `trust`, `workflow`. The eleventh, `crates/cli`,
  carries `#![deny(unsafe_code)]` — at both of its roots, `src/lib.rs` and
  `src/main.rs`. The difference is deliberate and narrow: `forbid` cannot be
  downgraded by a local `#[allow]`, and the crate needs exactly one. That
  single `#[allow(unsafe_code)]` — the only one in the workspace — sits on
  the `mod sys;` declaration in `src/lib.rs`, so the entire unsafe surface of
  the workspace is one greppable file, `crates/cli/src/sys.rs`: a handful of
  libc calls for signal delivery, process-group setup, one stdout fd dance,
  and a writability probe, each wrapped in a safe function, and the module
  itself stays crate-private. Don't add unsafe anywhere else. A second
  `#[allow(unsafe_code)]`, or a `forbid` weakened to `deny`, changes a
  reviewable property of the whole workspace — raise it in the PR
  description first.
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

## Submitting

The standard GitHub flow: fork the repository, branch from `main`, push the
branch to your fork, and open a pull request against `main`. Fill in the
[pull-request template](.github/PULL_REQUEST_TEMPLATE.md) — the checklist is
the same set of gates described above, and the body is where an enforcement
waiver or a new-dependency proposal has to appear.

There is **no DCO and no commit-signing requirement**: no `Signed-off-by`
trailer, no GPG or sigstore signature, no CLA. Nothing in the workflows checks
for one. Contributions are accepted under the repository's dual
MIT / Apache-2.0 licence
([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE)).

## Easiest first contribution

Adding a CLI adapter is one data-driven YAML descriptor — copy
`crates/adapters/descriptors/codex.yaml`, check it with
`agentstack adapters validate my-adapter.yaml`, and drop it into
`~/.agentstack/adapters/` to test without a rebuild.

## Reporting a vulnerability

See [`SECURITY.md`](SECURITY.md) — please use private reporting rather than
a public issue.
