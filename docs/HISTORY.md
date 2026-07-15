# AgentStack — History

Dated corrections and closed-out ledgers. Nothing here is current spec — it is
the record of *how* the current spec was reached, kept out of `ARCHITECTURE.md`
and the current planning sources so those read as the present tense. When a
claim here conflicts with `ARCHITECTURE.md`, `ENFORCEMENT.md`, `STRATEGY.md`,
`TODO.md`, or the code, those win; this file is memory, not authority.

## Dated corrections

### 2026-07-14 — D4 made the gateway the sole lockdown MCP authority

The lockdown run now resolves and pin-verifies one profile-fenced server set
and gives those same frozen definitions to both gateway dispatch and egress
classification. Every normalized declared HTTP MCP host enters
`gateway_only_hosts` in ruleset v3; that direct-deny wins over an ordinary
egress allow on every port. stdio upstreams remain host-side. Literal-IP and
non-TLS tunnels are refused, and partial, drifted, or unclassifiable resolution
fails the run.

There is no rendered-config fallback under lockdown. A trusted run must install
the token-bearing gateway entry and shadow native project/user configuration; an
empty or untrusted run must install empty shadows. If the adapter cannot carry
the gateway token or its config cannot be mapped and shadowed, the run refuses
to start. Docker tests prove a direct connection to a declared upstream fails
while the same call succeeds and is recorded through the relay. The precise
claim covers declared normalized endpoints, not every undeclared DNS alias the
same service may operate.

### 2026-07-14 — D1 made machine-policy corruption fail closed

Machine policy now has four honest operating states: absent/unconfigured;
current with a valid snapshot; degraded while enforcing the exact
last-known-good input after a broken edit; and blocked when both source and
snapshot are unusable. The snapshot stores a versioned, secret-free validated
machine `Policy` input plus source digest—not a project-specific effective
ruleset—and is refreshed atomically only when the digest changes. Gateway,
render, guard, explain, and sandbox planning all use the same result-returning
CLI provider, so substituting an empty policy on parse failure is no longer
representable.

This is resilience against accidental configuration rot, not tamper-proofing:
the snapshot is user-writable like the source manifest. Tests cover absent,
valid, unchanged-digest, degraded, first-run blocked, corrupt snapshot, future
version, and a gateway deny surviving source corruption.

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

**Standing rule it produced** (now stated timelessly in `CLAUDE.md`):
coupling claims made from combined grep sweeps are hypotheses. Before design work
is scheduled on an asserted "X depends on Y," verify it per-file against the
source.

### 2026-07-11 — the `adapters → policy` edge withdrawn

The crate-dependency table once granted `adapters → policy`, but the crate never
used it: the fail-closed secret check happens *before* render, in the caller. The
edge was withdrawn from CLAUDE.md and ARCHITECTURE.md to match reality (security
review L5, doc half). Re-granting it is a deliberate architecture change, not a
Cargo.toml edit.

### 2026-07-11 — enforcement-backed action evidence, not generic observability

An earlier positioning option proposed universal agent observability. It was
rejected because reliable cross-client observation requires essentially the
same interception boundary as enforcement while making a weaker promise.
AgentStack therefore owns portable evidence for actions it actually governs:
trust decisions, policy outcomes, capability use, lifecycle, and execution
receipts. It does not position itself as a tracing, token analytics, evaluation,
or payload-inspection platform. Future sequence-anomaly signals remain labelled
metadata heuristics, not DLP or content inspection.

## Security-review ledger (2026-07-11)

The two-reviewer audit (`docs/security-review-2026-07-11.html`), tracked to
closure. The current open items live in `TODO.md`; the closed detail lives here.

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
behavior-preserved. D4 subsequently made the frozen server definitions shared
authority for gateway dispatch and lockdown egress classification. Extending
the full resolved grant through the locked host run, native render, and profile
leases remains D2 work tracked in `TODO.md`.
</content>
