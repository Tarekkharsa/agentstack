# AgentStack — Enforcement matrix

This is the authoritative, code-grounded answer to one question: **for each
execution mode, what does AgentStack actually enforce, and by what mechanism?**
When any other document and this one disagree, this one is right — it is checked
against the source, not against intent.

## Claim discipline

AgentStack **restricts destinations and records decisions; it cannot guarantee
that sensitive content never leaves through an allowed destination.** An enforced
egress allowlist blocks connections to hosts you did not approve — it does not
inspect payloads, and it permits traffic to every host you *did* approve,
including the model API itself. A prompt-injected agent can still exfiltrate
through any allowed channel. The honest claim is: *untrusted code stays inert,
and unapproved egress is blocked* — never "exfiltration is impossible."

Read every cell below with that ceiling in mind. "Enforced" means the disallowed
action is *prevented at runtime* (by the kernel, the container boundary, or the
proxy); it never means the allowed action is *safe*.

## What "trusted" does and does not mean

Trusting a bundle asserts exactly one thing: **this exact content — this lockfile
digest, and in the distribution path this signature — was reviewed and approved.**
It is a claim about *bytes*, not about a machine, a user, or a runtime.

Trusted does **not** mean:

- **Safe to run unsandboxed.** Trust gates *whether* a bundle's servers spawn,
  its skills enter context, and its secrets resolve. It does not confine what a
  running agent then does — that is the job of policy and the sandbox, per the
  matrix below.
- **Vetted for correctness or intent.** Review shows you the diff (manifest,
  skill content, MCP definitions, policy) so *you* can judge it. AgentStack
  verifies the content is the content you approved; it does not vouch for what
  that content does.
- **Tamper-proof against a compromised host agent.** In host mode the agent CLI
  runs as you, so it can in principle reach the user-writable trust store under
  `~/.agentstack/` and self-trust a bundle. Only the sandbox removes this. (A
  recorder-backed tamper log for trust-store mutations is *intended* but not yet
  wired — see the audit/recording row.)

Conversely, **untrusted means fully inert**: no MCP server is spawned or
contacted, no skill content enters any agent context, no secret resolves, no
adapter config is written. That invariant *is* enforced (property-tested in
`crates/trust`).

## The matrix

Modes are columns; policy dimensions are rows. Legend:

- **enforced** — a runtime mechanism prevents the disallowed action; bypass means
  defeating the kernel, the container, or the proxy.
- **coarse** — a real check runs, but at coarser granularity than the policy can
  express (whole-workspace mount vs. per-path; host-only at write time vs. exact
  host:port at runtime; once-at-construction vs. per-call).
- **unsupported** — no code path on this mode consults the policy for this
  dimension; the dimension has **no effect** here. (Stated bluntly rather than
  softened to "advisory": for these cells no check happens at all, so there is
  nothing to bypass.)

| Dimension | `host` | `gateway` | `--sandbox` | `--lockdown` |
|---|---|---|---|---|
| **Tools** | unsupported | **enforced** | **enforced**† | **enforced**† |
| **Egress** | coarse | coarse | **enforced**\* | **enforced** |
| **Secrets** | **enforced** | **enforced** | **enforced**‡ | **enforced**‡ |
| **Filesystem — write** | unsupported | unsupported | coarse | coarse |
| **Filesystem — read** | unsupported | unsupported | coarse | coarse |
| **Audit / recording** | unsupported | **enforced** | **enforced**§ | **enforced**§ |

\* **for proxied traffic only.** Plain `--sandbox` points `HTTPS_PROXY` at the
proxy but the container keeps an ordinary bridge network — a process that
ignores the proxy env can still dial out directly. The run is labelled
`SANDBOX / PROXIED · DIRECT ROUTE OPEN` for exactly this reason; only
`--lockdown` (no direct route, topological confinement) earns `ENFORCED`.
See the egress section below.

† **for MCP traffic routed through the gateway.** `run --sandbox[/--lockdown]`
of a *trusted* bundle now renders one gateway HTTP entry into the harness's
config (host-side gateway; the sidecar relay bridges it under lockdown) and
shadows any direct project config, so tool calls hit `Gateway::try_call` and
`[policy.tools]` is enforced there. The ceiling matches egress's: an agent that
reaches an upstream host *directly* — any host on `--sandbox`'s open route, or
an egress-*allowed* host under `--lockdown` — bypasses the gateway. Denying the
container direct egress to upstream hosts (leaving the relay the only path to
them) is the remaining step for an unconditional cell; until then this is
"enforced for what transits the gateway," not "impossible to evade." An
*untrusted* bundle is not routed at all (empty gateway), so its cell is
`unsupported` — no tool policy, but also no secrets and no server spawn.

‡ **for gateway-routed runs.** The host-side gateway resolves `${REF}` secrets
in its own memory and hands the container only the endpoint URL + a per-run
bearer token — resolved secret *values* never enter the container. A prior
`agentstack apply` that baked secrets into a project config is shadowed out.
(A run that isn't gateway-routed falls back to the coarse rendered-config path.)

§ **tool calls + secret refs now recorded.** A gateway-routed run's own
`events.jsonl` gains a `ToolCall` per call (digest-only args) and a
`SecretAccess` per resolved ref (name only), alongside the lifecycle + egress
events it already held. Trust-store mutations and cost/tokens remain unrecorded.
See the Audit / recording section.

Two of the four columns are execution modes for a *rendered* config: **host** is
`agentstack apply` + `agentstack run` (adapters write native config, the harness
runs on the bare machine and talks to upstream MCP servers directly).
**gateway** is the in-process broker (`agentstack mcp`, `connect`, code mode) —
every MCP call routes through `Gateway::try_call`. **--sandbox** and
**--lockdown** are `agentstack run --sandbox [--lockdown]`: the harness runs in a
Docker container behind the egress proxy.

## Per-cell notes

### Tools

- **host — unsupported.** `render_server()` and `plan_target_with_servers()`
  write the manifest's servers straight into the harness's native MCP config with
  the real command/URL; the harness then talks to upstream servers directly.
  `CompiledRuleset::tool_decision` is never called on this path.
  (`crates/adapters/src/render.rs`, `crates/cli/src/render/apply.rs`)
- **gateway — enforced.** Every call checks `tool_decision(server, tool)` before
  dispatch, and `namespaced_tools()` filters denied tools out of discovery too, so
  a denied tool is invisible *and* refused if called anyway. This is the single
  enforcement point. (`Gateway::try_call`, `crates/cli/src/gateway.rs`)
- **sandbox / lockdown — enforced for gateway-routed traffic.** A trusted run
  builds a host-side gateway (`Gateway::from_plan` — hard trust gate: untrusted
  → empty → unrouted) and a token-gated HTTP MCP endpoint, then renders one
  gateway entry into the harness's user-scope config and shadows any direct
  project config. The container's MCP calls therefore reach `Gateway::try_call`,
  where `[policy.tools]` is enforced exactly as in gateway mode (denied at
  discovery *and* at call), and each call is recorded in the run's own
  `events.jsonl`. Under `--sandbox` the container reaches the gateway directly
  (`host.docker.internal`); under `--lockdown` — no host route — it reaches the
  egress sidecar's fixed-destination **relay** (`crates/egress/src/relay.rs`),
  which splices to the host gateway. The relay is a dumb byte pipe: it parses
  nothing, runs no policy, and does no SSRF check (its destination is host-fixed,
  not client-chosen), so the real egress proxy's anti-SSRF guard is untouched;
  auth stays end-to-end at the gateway. The proxy still never parses MCP
  JSON-RPC — enforcement is at the gateway, not the proxy. **Ceiling:** an agent
  that opens its own connection to an upstream host the egress policy allows
  bypasses the gateway; making the relay the *only* route to upstream hosts
  (deny their direct egress) is the remaining step. (`Gateway::from_plan`,
  `crates/cli/src/gateway_http.rs`, `crates/cli/src/commands/sandbox.rs`
  `wire_sandbox_gateway`, `crates/runtime/src/lockdown.rs`)

### Egress

- **host — coarse.** Write/spawn-time check only: for HTTP servers the declared
  URL host is extracted and `egress_decision(name, host, None)` is called with
  **port `None`** when the config is written. A host hidden behind an unresolved
  `${REF}` fails closed only if the server is egress-constrained. There is no
  runtime traffic filtering once the harness is running natively.
  (`crates/cli/src/render/apply.rs`)
- **gateway — coarse.** For HTTP upstreams the resolved host is checked once at
  construction (`egress_decision(name, host, None)`, port `None`); a constrained
  server whose host can't be determined is skipped. There is no per-call egress
  re-check, and stdio (child-process) upstreams get no egress check at all — their
  network access is unconstrained by AgentStack. (`crates/cli/src/gateway.rs`)
- **sandbox — enforced.** Every CONNECT is checked against `EgressGuard::decide`
  with the **real port** from the CONNECT line; resolved addresses must be global
  unicast (anti-SSRF, `netguard`); the TLS ClientHello SNI must equal the CONNECT
  host (anti-domain-fronting); a per-run token gates who may use the proxy at all.
  **Topology caveat:** `--sandbox` gives the container an ordinary bridge network
  with `HTTPS_PROXY` pointed at a host proxy — a container that *ignored*
  `HTTPS_PROXY` could still reach the open internet directly. Egress is enforced
  for traffic that goes through the proxy, not guaranteed the way `--lockdown` is.
  (`crates/egress/src/proxy.rs`, `crates/cli/src/commands/sandbox.rs`)
- **lockdown — enforced (topological).** The container is attached ONLY to an
  internal Docker network whose sole reachable peer is the egress-proxy sidecar;
  there is no host route, no internet, no DNS beyond it. Ignoring the proxy env
  reaches *nothing*. The sidecar runs the identical `ServerProxy` enforcement as
  `--sandbox`. This is strictly stronger: confinement is topological, not
  convention. (`crates/runtime/src/lockdown.rs`, `crates/egress/src/proxy.rs`)

### Secrets

- **host — enforced, fail-closed.** `ScopedResolver::resolve` calls
  `secret_decision(server, name)` before returning any value; a denied or
  unresolvable `${REF}` blocks the write rather than emitting a literal
  placeholder. Once allowed, the concrete value is written into the native config
  file on disk — that on-disk exposure is a separate, accepted fact (ARCHITECTURE
  Layer 1), not a policy gap. (`crates/cli/src/secret/mod.rs`)
- **gateway — enforced, fail-closed.** A per-server `ScopedResolver` substitutes
  every `${REF}` through `secret_decision`; a ref outside `[policy.secrets]` fails
  to resolve, and the call is refused outright if any refs remain unresolved for
  that server. Same mechanism as host mode. (`crates/cli/src/gateway.rs`)
- **sandbox / lockdown — coarse.** The sandbox path never resolves or injects
  secrets itself. Secrets reach the container the same way as host mode: a prior
  `agentstack apply` on the host resolved `${REF}`s fail-closed (the enforced host
  mechanism) and wrote literal values into the config under the project directory,
  which is then bind-mounted into the container. The fail-closed check *is*
  enforced (it's the host path), but there is no per-run, per-server secret
  injection specific to the sandbox — it inherits whatever was already written to
  disk and mounts it. (`crates/cli/src/commands/sandbox.rs`)

### Filesystem — write

- **host / gateway — unsupported.** No sandbox, no mount, no path-scoping touches
  either path. `runs.rs` spawns the harness against the real filesystem; the
  gateway is an in-process broker with no mount logic. `workspace_write_decision`
  is never consulted, and stdio MCP children run with the ambient user's full
  filesystem permissions. (`crates/cli/src/runs.rs`, `crates/cli/src/gateway.rs`)
- **sandbox / lockdown — coarse.** The whole workspace is one bind mount, mounted
  `:ro` unless the effective write scope covers the workspace root
  (deny-by-default — the one dimension where absence means deny). A partial scope
  like `src/**` rounds *down* to read-only, since it's one all-or-nothing mount.
  The kernel enforces the `:ro` bind, not the harness. Coarse by definition:
  whole-workspace, not per-path. (`crates/cli/src/commands/sandbox.rs`,
  `CompiledRuleset::workspace_write_decision` in `crates/policy/src/ruleset.rs`)

### Filesystem — read

- **host / gateway — unsupported.** Same paths as filesystem-write; `FsRules.read`
  is never consulted. (`crates/cli/src/runs.rs`, `crates/cli/src/gateway.rs`)
- **sandbox / lockdown — coarse.** The whole workspace is visible inside the
  container and nothing outside the mounted workspace directory is — so the
  workspace boundary itself is a real, kernel-level read scope. But no finer mount
  is created from `[policy.filesystem] read`, so read globs narrower than the whole
  workspace are informational only. (`crates/cli/src/commands/sandbox.rs`,
  `crates/runtime/src/spec.rs`)

### Audit / recording

- **host — unsupported.** Native host-mode runs never call `calllog::record`
  because the harness talks to upstream MCP servers directly, bypassing AgentStack
  entirely. Audit happens only if the harness is separately configured to route
  via the gateway (`agentstack mcp`). (`crates/cli/src/runs.rs`)
- **gateway — enforced.** `Gateway::try_call` logs every outcome (denied / ok /
  error) via `calllog::record` to `~/.agentstack/audit/calls.jsonl`. Only an
  argument *digest* is stored, never raw values or resolved secrets, and upstream
  error text is reduced to a fixed class so a malicious upstream can't write
  arbitrary bytes into the log. This is the most complete audit dimension.
  (`crates/cli/src/gateway.rs`, `crates/recorder/src/lib.rs`)
- **sandbox / lockdown — enforced (for a gateway-routed run).** `RunLog::create`
  is mandatory and fails closed ("nothing trusted runs unobserved"). The run log
  captures container lifecycle (`SandboxStarted` / `SandboxExited`) and every
  egress decision — and, now that a trusted run's MCP traffic routes through the
  host-side gateway, every **tool call** (`ToolCall`: server, tool, outcome,
  argument *digest* only — never values) and every **secret reference** resolved
  (`SecretAccess`: ref *name* only). The gateway mirrors these into the run's own
  `events.jsonl` because it inherits the run id (`Gateway::from_plan`), so
  `agentstack report <run>` reads a self-contained record without the
  cross-project audit log. Still missing from a run's log: trust-store mutations
  (below) and cost/tokens. A run that isn't gateway-routed (untrusted bundle, or
  no servers) records only lifecycle + egress. (`crates/cli/src/gateway.rs`
  `log_call`, `crates/cli/src/commands/sandbox.rs`)

### Not yet wired: trust-store mutation logging

ARCHITECTURE Layer 2 describes logging every trust-store mutation as tamper
evidence, as mitigation for the host-mode self-trust risk. **As of this writing
that is intended, not implemented** — the trust command and `crates/trust` call no
recorder. Treat it as a planned mitigation, not a shipped guarantee, until a
run/audit event for trust mutations exists.

## See also

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — the layer model this matrix concretizes,
  especially Layer 3 (policy dimensions) and Layer 4 (runtime modes).
- [`ROADMAP.md`](ROADMAP.md) — what remains (the run-log fill-out for the coarse
  audit cells; distribution/signing).
- [`HISTORY.md`](HISTORY.md) — dated corrections and the closed security-review
  ledger.
