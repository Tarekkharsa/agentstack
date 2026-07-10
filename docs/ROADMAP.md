# AgentStack — Roadmap

Build strictly in phase order. Do not scaffold future phases early — the
boundaries in ARCHITECTURE.md exist so later phases slot in without rework.

Standing context: the shipped v0.8.x binary already implements v0 of the
trust gate, policy, audit log, secrets, and all 13 adapters. Phases 0–1 are
**extraction and hardening** of that code, not new construction. There are no
external users — breaking changes to formats, paths, and CLI surface are free.
No migration shims.

## Session types — supervision policy

Classify every piece of work before starting it. The split is about how a
mistake gets caught: machine-checkable work can run long and unattended;
security-semantic work gets short sessions and line-by-line human review.

**Long-run eligible** (large unattended sessions are fine — correctness is
machine-checkable or review is cheap):

- Adapter coverage for additional agent CLIs, with conformance tests
- Expanding the proptest suites (more generators, more invariant instances)
- The Phase 2 malicious-repo demo PoC
- Bundle round-trip conformance tests
- Documentation
- The run-report viewer (Phase 3)
- (post-demo backlog) `doctor` lint: warn when the machine policy carries
  server-specific deny rules with no `"*"` fallback — a named deny can be
  dodged by a repo renaming its server; the wildcard form is rename-proof

**Supervised only** (short sessions, plan-first, maintainer reviews line by
line):

- Anything touching trust granting
- Policy composition / the intersection engine
- Secret resolution
- Digest computation
- The `adapter::render` / `resolve` seam design

**Near-term order:** the adapter seam gets settled first, in a supervised
session. After that, the next *large* session targets the demo PoC repo and
adapter-matrix expansion — the wedge and the demo take priority over further
foundation work, which is now closed except for what this roadmap already
lists.

## Phase 0 — Extraction (the restructure)

Step 1: convert to a virtual workspace with the whole existing crate moved to
`crates/cli` unchanged (embedded `adapters/`/`catalog/` paths adjusted, tests
green). Then carve crate by crate, moving code — not rewriting it (mapping
verified against the source, 2026-07-10):

- `src/manifest/`, `src/lock.rs`, `src/util/` (shared path helpers), digest
  code → `core`, plus two pieces the compiler flushed out: `scope.rs` (the
  layering enum the model references) and the pure `${REF}` syntax scanner
  (`refs_in`/`is_ref_name` — resolution stays in `cli`). The requested-policy
  schema (`manifest::Policy`) stays with manifest parsing in core — it is
  manifest data. `manifest::validate` stays in `cli` (it walks the library
  and resolver), re-exported so callers still see one module. `core`
  temporarily keeps `clap` (two enums derive `ValueEnum`) — dropping it is
  Phase 1 hardening.
- `src/trust.rs` (trust store, digest pinning) → `trust`; its CLI command
  (`src/commands/trust.rs`) stays in `cli`. Coupling is clean: lock + two
  manifest constants + `util::paths` only.
- There is no policy module today: the machine-policy loader
  (`manifest::machine_policy`) and the `[policy.tools]` checks inline in
  `gateway.rs`/`mcp_server.rs` → `policy`, as the seed of the intersection
  engine. Enforcement call sites stay in `cli` and call into it.
- `src/adapter/` + the 13 `adapters/*.yaml` (and their `include_dir` embed +
  build.rs rerun) → `adapters`. **Correction (2026-07-10):** this bullet
  originally claimed `adapter::render` depends on `resolve`/`library`/`store`
  and called it "the one non-mechanical seam." Per-file reading showed that
  was a grep artifact — those imports belong to `manifest/validate.rs`; the
  adapter module's only non-core dependency was the `Resolver` trait, which
  moved to `core::secret`. The pipeline (`render/apply.rs`) already resolves
  before it renders; the boundary followed the data all along.
  **Meta-lesson, standing rule:** coupling claims made from combined grep
  sweeps are hypotheses. Before design work is scheduled on an asserted
  "X depends on Y," the session verifies it per-file against the source.
- `src/calllog.rs` (audit-log writer) → `recorder`, seeding the event types.
- Everything else (library, plugins, dashboard, analyze, codemode, the
  observation proxy, resolve/store) stays in `cli` for now.
- Leave `runtime` and `egress` uncreated until Phase 2.

`#![forbid(unsafe_code)]` in every crate. Moved code may keep `anyhow` until
the Phase 1 thiserror conversion — rule 6's strict list for `trust`/`policy`
is enforced from Phase 1 on. CI: fmt + clippy(-D warnings) + test.

Done when: the workspace compiles, the existing test suite passes, and the
binary still works end to end.

## Phase 1 — Trust core (harden the standalone product)

1. `core`: settle the bundle format starting from the shipped
   `agentstack.toml` (semantics per ARCHITECTURE Layer 1; breaking changes
   fine). Defensive parsing: size bounds, unknown-field rejection.
2. `trust`: extend pinning from manifest + lockfile to **content pinning** of
   everything referenced — skills, instructions, scripts — closing the v0.8.x
   gap where an edit to a referenced file did not re-gate. `agentstack review`
   diff rendering (manifest, skill content, MCP defs, policy).
   Property test: any single-byte change in any pinned file → untrusted.
3. `policy`: **done.** Generalized the machine-first tool check into a real
   (machine ∩ bundle) intersection engine; added `[policy.egress]`,
   `[policy.secrets]`, and `[policy.filesystem]` dimensions alongside
   `[policy.tools]`, all sharing one glob grammar and rename-proof `"*"` key;
   compiled the two-layer result into a serializable `CompiledRuleset`
   (`crates/policy`), with per-dimension property tests (`effective(B, M) ⊆
   M`, for all inputs, never deleted or weakened). Secret access is enforced
   fail-closed at both substitution sites (adapter render + gateway
   resolver); egress is enforced against each server's declared host at
   write/spawn time. Filesystem scopes are carried and compiled but stay
   advisory until Phase 2's sandbox mounts.
4. `adapters`: already shipped (13 CLIs, data-driven YAML) — keep behavior,
   verify blocked writes when any `${REF}` is unresolved (keychain/varlock,
   fail closed).
5. `agentstack init` (already shipped): confirm it produces a valid bundle
   under the settled format.

Done when: clone a bundle repo → it is inert → review → trust → configs
materialize for 2+ CLIs, with content pinning and both property tests green.
Ship this. Announce this.

## Phase 2 — Enforcement (sandbox + egress proxy)

0. Prerequisite: recorder event types + a minimal JSONL sink exist (extracted
   in Phase 0) so proxy decisions have somewhere to land — the report viewer
   waits until Phase 3.
1. `runtime`: container lifecycle via `bollard` — create, mount workspace,
   no-network namespace except the proxy route, stream output, teardown.
2. `egress` (first async crate — budget learning time, and know that the
   proxy's *design* is harder than the async: per-server egress attribution
   needs one proxy identity per server; HTTPS allowlisting means CONNECT/SNI
   filtering, no MITM; DNS must be routed and filtered too): **consumes the
   `CompiledRuleset` artifact** produced by `crates/policy` (the identical
   value the gateway already reads, serialized across the process boundary —
   no re-deriving policy in the proxy); allow/block per host per MCP server
   using its `egress_decision`; one event per decision into the recorder
   sink. Filesystem scopes in the same ruleset become enforceable here too,
   via the sandbox mounts (item 1) rather than the proxy.
3. `agentstack run --sandbox <bundle>`.
4. **The demo:** a benign proof-of-concept "malicious" repo that phones home /
   reads a fake secret when used unprotected, and sits inert at the trust
   gate + gets blocked by the proxy under AgentStack. This demo is the pitch.
   Claim exactly what it proves — *unreviewed repos stay inert; unapproved
   egress is blocked* — never "exfiltration is impossible": a prompt-injected
   agent can still leak through allowed hosts, including the model API.

Done when: the PoC attack demo works end to end, recorded.

## Phase 3 — Flight recorder surface

- `recorder`: fill out the run log (tool calls + args, blocks, secret refs
  touched, trust-store mutations, cost/tokens, wall time) fed by egress +
  adapter + CLI stream events.
- `agentstack report <run>`: readable run report.
- Keep scope: log + viewer. No dashboards.

Done when: every sandbox run produces a report a security reviewer could read.

## Phase 4 — Distribution (only after real users)

- ed25519 signing of lockfiles; `agentstack verify`.
- Curated Git repo of signed bundles as registry v0.
- Real registry infrastructure only when pull numbers justify it.

## Parallel track — Rust learning ladder

Session order chosen so language difficulty ramps with the roadmap:
workspace surgery & module visibility (Phase 0) → digests & serde (1.1–1.2) →
traits & error enums (1.3) → cross-crate design (1.4–1.5) →
processes & Docker API (2.1) → async/tokio (2.2, the hardest step — take it
slow, it is the last one).
