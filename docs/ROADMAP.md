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
   write/spawn time. Filesystem write scopes are enforced by the Phase 2
   sandbox's workspace mount (read-only unless covered, deny-by-default;
   Docker-verified in `sandbox_fs.rs`); read scopes stay informational, and
   host mode enforces neither.
4. `adapters`: already shipped (13 CLIs, data-driven YAML) — keep behavior,
   verify blocked writes when any `${REF}` is unresolved (keychain/varlock,
   fail closed).
5. `agentstack init` (already shipped): confirm it produces a valid bundle
   under the settled format.

Done when: clone a bundle repo → it is inert → review → trust → configs
materialize for 2+ CLIs, with content pinning and both property tests green.
Ship this. Announce this.

## Phase 2 — Enforcement (sandbox + egress proxy)

Status: every component is built and tested to the limit of a Docker-less
environment (654+ tests, loopback-verified where a daemon isn't needed). The
only remaining work is behavior-verification against a real Docker daemon —
the container↔proxy routing and the recorded demo — flagged per item below.

0. **[done]** `recorder`: `RunEvent` + `RunLog` — a per-run `events.jsonl` sink
   for lifecycle + egress decisions (the report viewer is Phase 3).
1. **[done, bollard behavior gated]** `runtime`: `Sandbox` trait +
   orchestrator (create, mount workspace, no-network, stream output,
   teardown), unit-tested against a fake; a `bollard` backend behind an opt-in
   `docker` feature, compile-verified with a daemon-gated integration test.
   *Remaining (Docker):* the `NetworkPolicy::ProxyOnly` container wiring.
2. **[done, loopback-verified]** `egress` (tokio confined here): CONNECT-target
   + TLS-SNI parsing (bounds-checked); `EgressGuard` consumes the
   **`CompiledRuleset` artifact** (the identical value the gateway reads) and
   decides allow/block per host per server, one event per decision; the async
   `ServerProxy` (one per server → per-server attribution) tunnels allowed
   CONNECTs and refuses blocked ones; `EgressBridge` stands up the per-server
   set. DNS is gated implicitly (the proxy resolves only allowed hosts).
   Verified end to end on loopback AND against a real container (item 4).
   `BlockingBridge` is the sync facade the cli drives (tokio stays in egress).
   **Destination hardening (security review follow-up):** hostnames are
   normalized (lowercase + trailing-dot strip) before matching; the parsed SNI
   is *enforced* to equal the CONNECT host, and an incomplete ClientHello fails
   closed (no domain fronting); resolved addresses are checked against an
   IP-class guard (`netguard`) that permits only global unicast — loopback,
   private, link-local (incl. the `169.254.169.254` metadata IP), unique-local,
   and reserved ranges are refused, and the proxy dials the validated address
   (no re-resolution → no DNS rebind), so a literal-IP or SSRF pivot into the
   host/internal network is blocked. Per-step timeouts bound slowloris. A
   per-run Basic-auth token authenticates the sandbox to the proxy (the listener
   must bind a broad address, so the token — not the bind — is what stops an
   open relay; a CONNECT without it gets 407). The `CompiledRuleset` version is
   checked at the enforcement boundary and fails closed when newer than the
   proxy understands.
3. **[done]** `agentstack run --sandbox <bundle>`: builds the `SandboxSpec`
   (tested), stands up the egress proxy from the effective policy, injects
   `HTTPS_PROXY` into the container, and records lifecycle + egress decisions
   to the run log (readable via `agentstack report`). Execution behind the cli
   `sandbox` feature; verified through the real binary on Docker. The
   workspace mount enforces the `[policy.filesystem]` write scope: read-only
   unless the effective scope covers the workspace (deny-by-default), the
   `:ro` bind enforced by the kernel — Docker-verified through the real
   binary in `crates/cli/tests/sandbox_fs.rs`.
4. **[done — verified on real Docker]** The demo, proven two ways on Docker
   25.0.3: (a) a real `curl` container exfiltrates to a host reachable only
   through the proxy — blocked under a deny policy (sink gets nothing), tunneled
   under allow (sink gets it), the machine policy deciding
   (`crates/cli/tests/sandbox_egress.rs`); (b) driven through the real
   `agentstack run --sandbox` binary, whose flight recorder shows the egress
   BLOCK (`crates/cli/tests/sandbox_cli_e2e.rs`). The bollard backend's
   create→stream→teardown is likewise verified against a live daemon. Claim
   exactly what it proves — *unreviewed repos stay inert; unapproved egress is
   blocked* — never "exfiltration is impossible": a prompt-injected agent can
   still leak through allowed hosts, incl. the model API.

Done when: **met** — the PoC attack demo works end to end, both directly and
through the real CLI (verified live on Docker 25.0.3).

**No-direct-route lockdown — DONE, Docker-verified.** `run --sandbox` gives
the container a bridge network and points its `HTTPS_PROXY` at a host-process
proxy: enforces the agent's *configured* egress and blocks anything reachable
only via the proxy, but a container that ignored the proxy env could still
reach the open internet directly. `run --sandbox --lockdown` closes that: the
container is attached ONLY to a Docker `--internal` network (no host route, no
internet, no DNS beyond it) whose single reachable peer is the egress proxy
running as a **sidecar container** — dual-homed onto a second ordinary network
so it (and only it) forwards allowed traffic out. Ignoring the proxy env then
reaches nothing; the confinement is topological. Shipped as: the
`agentstack-egress-proxy` binary + `docker/egress-proxy.Dockerfile`, the
`runtime::docker::Lockdown` orchestrator (creates both networks + the sidecar,
follows its RunEvent stream into the run log, tears everything down on drop),
and the `--lockdown` flag. Verified live on Docker through the real binary:
`crates/cli/tests/sandbox_lockdown.rs` (a direct route bypassing the proxy env
reaches nothing; a proxied request to a denied host is blocked and recorded)
and `crates/egress/tests/sidecar_image.rs` (the image itself, incl. fail-closed
on a future ruleset version).

**Security-review ledger (2026-07-11,
`docs/security-review-2026-07-11.html`).** The two-reviewer audit's findings,
tracked to closure so this plan and the report can't drift apart:

- **Closed — Highs (H1–H5):** lockdown network (H1), hostname normalization
  (H2), enforced SNI-equals-CONNECT (H3), anti-SSRF IP-class guard +
  dial-the-validated-address (H4), literal-IP CONNECTs through the same guard
  (H5).
- **Closed — Mediums:** ruleset version gate fails closed at the decision
  boundary and in the sidecar (M1); length-framed trust digest segments (M2);
  single-write atomic run-log appends (M3); per-run proxy peer-auth token —
  the broad bind is no longer an open relay, and the same token authenticates
  the sandbox to the lockdown sidecar (M4); per-step proxy timeouts (M6);
  v2 dir digests skip symlinks and cap recursion depth (M7).
- **Closed — Lows:** bounded reads for hostile manifest/lockfile input plus
  hostile-input tests for the `${REF}` scanner (most of L4); digest paths
  hashed as raw bytes with `/` separators on unix (L2, unix half); the stale
  `adapters → policy` dependency edge withdrawn from CLAUDE.md and
  ARCHITECTURE.md (L5, doc half); a sandbox run now fails closed if its run log
  can't be created — "nothing trusted runs unobserved" (L4 remainder).
- **Closed — M5 (port-scoped egress):** egress patterns gained an optional
  `:port` (`api.example.com:443` scopes to that port; a bare host still means
  any port), matched by a shared `egress_match` threaded through the same
  intersection check so tools/secrets are untouched; the proxy enforces the
  exact CONNECT port, write-time checks defer the port to runtime. The grammar
  change bumped `RULESET_VERSION` to 2 so an older enforcer fails closed rather
  than misread `!host:port`. An adversarial verification pass (a Codex review +
  three invariant checkers) proved (machine ∩ bundle) still only narrows, drove
  the version bump, and closed two parser holes (malformed bracketed patterns no
  longer widen to any-port; port 0 is refused at CONNECT).
- **Open, accepted and tracked:** the stat-based digest
  cache stays off every verification path — that containment IS the fix, keep
  it true (L1); the event-sink append is synchronous inside the async proxy
  (L3 — latency, not correctness); `trust`
  still carries `anyhow` + `toml` (L5 code half, TODO-tracked for the Phase 1
  rule-6 sweep).
- **Structural recommendation, unscheduled:** the reviewers' "one
  enforcement-plan boundary" — a library API that turns a trusted, resolved
  bundle into one immutable plan that commands execute or display, instead of
  re-assembling the security model per command in the large `cli` files.
  Worth its own design session before Phase 3 grows the surface further.

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
