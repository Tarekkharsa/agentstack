# AgentStack — History

Dated corrections and closed-out ledgers. Nothing here is current spec — it is
the record of *how* the current spec was reached, kept out of `ARCHITECTURE.md`
and `ROADMAP.md` so those read as the present tense. When a claim here conflicts
with `ARCHITECTURE.md`, `ENFORCEMENT.md`, or the code, those win; this file is
memory, not authority.

## Dated corrections

### 2026-07-13 — governed ephemeral execution landed experimentally

The earlier capability-layer proposal was implemented as a machine-opt-in MCP
primitive, `tools_execute`, behind the sandbox build feature. A new
policy-agnostic `executor` crate freezes request bytes, exact grants, limits,
runtime identity, and digests; asynchronous framed transport lives in `egress`;
the CLI alone composes both with the existing gateway; and the recorder now
attributes child calls to execution IDs. The Docker backend gained non-root,
read-only-root, tmpfs, capability, CPU, memory, PID, timeout, output, and
teardown controls.

The claim was deliberately kept experimental. The implementation pins the
official Node image by digest and passes focused real-Docker isolation, relay,
timeout, output, trust, and advertisement tests. A focused implementation
review and regression-hardening pass completed on 2026-07-13. It does not yet
ship a project-specific executor image attestation/SBOM or longer-running soak
evidence; `ENFORCEMENT.md` records those limits rather than calling the feature
production-ready.

### 2026-07-10 — the `adapter::render` coupling was a grep artifact

An early extraction plan claimed `adapter::render` depended on
`resolve`/`library`/`store` and called it "the one non-mechanical seam."
Per-file reading disproved it: those imports belonged to
`manifest/validate.rs`, not the adapter module. The adapter module's only
non-core dependency was the `Resolver` trait, which moved to `core::secret`, and
the render pipeline (`render/apply.rs`) already resolves before it renders — the
boundary followed the data all along. This is why the `adapters` crate depends on
`core` only.

**Standing rule it produced** (now stated timelessly in ROADMAP Phase 0):
coupling claims made from combined grep sweeps are hypotheses. Before design work
is scheduled on an asserted "X depends on Y," verify it per-file against the
source.

### 2026-07-11 — the `adapters → policy` edge withdrawn

The crate-dependency table once granted `adapters → policy`, but the crate never
used it: the fail-closed secret check happens *before* render, in the caller. The
edge was withdrawn from CLAUDE.md and ARCHITECTURE.md to match reality (security
review L5, doc half). Re-granting it is a deliberate architecture change, not a
Cargo.toml edit.

## Security-review ledger (2026-07-11)

The two-reviewer audit (`docs/security-review-2026-07-11.html`), tracked to
closure. ROADMAP keeps a one-line summary and the still-open items; the closed
detail lives here.

### Closed — Highs (H1–H5)

- **H1** — lockdown network: the container attaches only to an internal Docker
  network whose sole peer is the egress-proxy sidecar (no host route, no
  internet, no DNS beyond it).
- **H2** — hostname normalization: hosts are lowercased and trailing-dot-stripped
  before matching, so casing can't dodge a deny.
- **H3** — enforced SNI-equals-CONNECT: the TLS ClientHello SNI must equal the
  CONNECT host, and an incomplete ClientHello fails closed (no domain fronting).
- **H4** — anti-SSRF IP-class guard + dial-the-validated-address: resolved
  addresses must be global unicast (loopback/private/link-local incl.
  `169.254.169.254`/unique-local/reserved refused), and the proxy dials the
  validated address with no re-resolution (no DNS rebind).
- **H5** — literal-IP CONNECTs flow through the same guard.

### Closed — Mediums (M1–M7)

- **M1** — ruleset version gate fails closed at the decision boundary and in the
  sidecar (a `CompiledRuleset` newer than the enforcer understands is rejected,
  not misread).
- **M2** — length-framed trust digest segments.
- **M3** — single-write atomic run-log appends.
- **M4** — per-run proxy peer-auth token: the broad listener bind is no longer an
  open relay, and the same token authenticates the sandbox to the lockdown
  sidecar; a CONNECT without it gets 407.
- **M5** — port-scoped egress (see below).
- **M6** — per-step proxy timeouts (bound slowloris).
- **M7** — v2 directory digests skip symlinks and cap recursion depth.

### Closed — Lows

- **L2 (unix half)** — digest paths hashed as raw bytes with `/` separators on
  unix.
- **L4** — bounded reads for hostile manifest/lockfile input, hostile-input tests
  for the `${REF}` scanner, and a sandbox run now fails closed if its run log
  can't be created ("nothing trusted runs unobserved").
- **L5 (doc half)** — the stale `adapters → policy` edge withdrawn (see the dated
  correction above).

### Closed — M5 (port-scoped egress)

Egress patterns gained an optional `:port` (`api.example.com:443` scopes to that
port; a bare host still means any port; `host:*` is explicit any-port), matched
by a shared `egress_match` threaded through the same intersection check so
tools/secrets are untouched. The proxy enforces the exact CONNECT port;
write-time checks defer the port to runtime. The grammar change bumped
`RULESET_VERSION` 1 → 2 so an older enforcer fails closed rather than misread
`!host:port`. An adversarial verification pass (a Codex review + three invariant
checkers) proved (machine ∩ bundle) still only narrows, drove the version bump,
and closed two parser holes (malformed bracketed patterns no longer widen to
any-port; port 0 is refused at CONNECT).

### Structural recommendation — done for the sandbox run path

The reviewers' "one enforcement-plan boundary" now exists for `run --sandbox`.
`ExecutionPlan::build` is the single seam that assembles a run — checks trust
(the previously-missing "verified content identity"), compiles the effective
(machine ∩ bundle) policy, resolves mounts + command, picks the egress mode — and
returns one immutable plan. Commands `execute` it (one fail-closed run log + one
proxy token, created once, no per-mode duplication) or `display` it (`--plan`, a
Docker-free dry run naming trust state, mode, and the exact command). Three
independent reviewers (Codex + two workflow checkers) confirmed the run is
behavior-preserved. Extending the same boundary to the host-mode run and the
gateway remains future work (tracked in ROADMAP).
</content>
