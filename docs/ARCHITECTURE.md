# AgentStack — Architecture

## Vision

AgentStack packages, runs, and governs AI agents — skills, tools, MCP servers,
and ephemeral generated capabilities — as trusted, portable bundles.

The strategic frame: the **agent bundle** is the standard unit (the way the image was Docker's unit). Everything else in the system gates it, constrains it, records it, or distributes it. Config unification across agent CLIs is the adoption wedge; the trust gate, firewall, and audit trail are the durable value. The registry/marketplace is the endgame, only viable because trust and signing exist first.

Core principle: **nothing executes automatically until its content is trusted;
governed execution is constrained and recorded.**

## Where this starts

The current v0.10.1 implementation is a nine-crate Rust workspace. It ships the
manifest and lock resolver, 13 CLI adapters, a central capability library,
content-bound trust, machine-first policy, a single-dispatch MCP gateway,
Docker sandbox and lockdown runtimes, egress enforcement, per-run recording,
and an experimental frozen-plan executor. This document describes the current
boundaries. [`ROADMAP.md`](ROADMAP.md) names what remains future work.

## The flow

```
manifest + library → resolve + lock → adapters → native CLI config
                            │
                            └→ trust → policy → gateway/runtime → recorder
                                            ↑
                                      machine rules
```

Static config compilation and governed execution are sibling paths. A normal
`apply` is an explicit, non-executing render operation; trust gates automatic
project loading and execution paths, not every config write.

Generated code follows the same path through a policy-agnostic execution
domain: the CLI freezes an exact tool grant and limits into an immutable plan;
the executor runs it inside the sandbox; and every capability call returns to
the existing gateway. The executor never reads or interprets policy. The
gateway remains the sole tool authority, the runtime owns isolation, the
egress crate owns asynchronous relay transport, and the recorder owns evidence.

A bundle arrives (cloned, pulled, copied) and is inert by construction.
`agentstack trust` displays its declared runtime surface, verifies its lock, and
pins the current manifest/local/lock digest in the machine-local trust store.
At run time, the policy engine intersects the bundle's requested policy with
the machine's rules and compiles the effective ruleset. In sandbox mode the
CLI's configured HTTP(S) traffic goes through the enforcing proxy; lockdown
makes the proxy sidecar topologically the only route out. Lifecycle, limit,
egress, brokered tool-call, and secret-reference events enter the per-run log.

## Layer 1 — The bundle (`crates/core`)

A bundle is a directory. It is **declarative and inert**: pure data, nothing executes.

```
my-agent/
  .agentstack/
    agentstack.toml        # preferred manifest
    agentstack.local.toml  # optional gitignored overlay
    agentstack.lock        # resolved, content-pinned inputs
    instructions/         # instruction files
    skills/               # skill directories (untrusted input)
```

Minimal `agentstack.toml` sketch:

```toml
version = 1

[servers.web-search]
type = "stdio"
command = "npx"
args = ["-y", "@example/search-mcp"]
env = { SEARCH_API_KEY = "${SEARCH_API_KEY}" }

[skills.summarize]
path = "./skills/summarize"

[instructions.team]
path = "./instructions/team.md"

[policy.tools]
web-search = ["*", "!*_delete"]
```

`agentstack.lock` pins resolved server definitions, skill-directory content,
and instruction bytes to SHA-256 digests. Trust separately binds the manifest,
local overlay, and lockfile into one consent digest. Detached ed25519 signing
and verification of the lockfile are available as distribution primitives.

Key decisions:
- Skills and instructions are content-pinned like code because they can alter
  agent behavior. Inline skills cannot be trusted until they are lock-pinned;
  library server drift likewise blocks trust and governed execution.
- Secrets appear only as `${REF}` placeholders, resolved by the OS keychain
  (`keyring`) or varlock. Resolution happens in memory at run time.
  Unresolvable secret → fail closed.

## Layer 2 — Trust gate (`crates/trust`)

Machine-local trust store: `canonical project path → trusted consent digest +
timestamp`. Publisher signatures are verified separately from this local
consent record.

The implemented states are **untrusted** and **trusted**. Before confirmation,
`agentstack trust` summarizes the exact stdio commands, HTTP contacts, secret
references, and skill pin status. Trust binds to the consent digest, so a
manifest, local-overlay, or lockfile change re-gates automatically. Automatic
project loading and experimental execution refuse untrusted content; an
explicit static `apply` remains a separate user-authorized operation.

Invariant: changing any byte in the manifest/local/lock consent surface changes
the trust digest. Changing lock-pinned skill, instruction, or library-server
content fails lock verification until the project is deliberately re-locked
and re-trusted.

**Principle — content identity and local consent are separate.** The consent
digest is content-shaped, but the trust decision is deliberately stored under
the project's canonical path on one machine. Detached signatures provide the
portable claim: a maintainer signs lockfile bytes, CI or a recipient verifies
them, and the recipient still makes its own local trust decision. Hostnames and
usernames never enter the content digest.

**Honest limitation:** the trust store and machine policy live under
`~/.agentstack/`, which is writable by the user — and in host mode the agent
CLI runs *as* the user, so a compromised agent could modify them and
self-trust a bundle. Only sandbox mode removes this. The intended mitigation —
having the recorder log every trust-store mutation as tamper evidence — is
**not yet wired** (the trust command and `crates/trust` call no recorder today);
until it is, treat it as planned, not a shipped guarantee. See
[`ENFORCEMENT.md`](ENFORCEMENT.md) for the exact per-mode enforcement status.

This layer must work standalone — valuable with no sandbox, no registry.

## Layer 3 — Policy engine (`crates/policy`)

Two inputs: the bundle's requested policy (`[policy.*]` in its manifest) and
the machine policy — the `[policy.*]` tables of the machine-local
`~/.agentstack/agentstack.toml` manifest (TOML, loaded by
`manifest::machine_policy()`; not a separate `policy.yaml`). The machine
policy lives outside every repo's tree, so no repo content can alter it — but
see the host-mode limitation in Layer 2: it is still a user-writable file.

Output: effective policy = **intersection**. Bundles can narrow, never widen.
(The shipped machine-first `[policy.tools]` check is the v0 of this rule;
Phase 1 generalized it into a real intersection engine with more dimensions.)

Four dimensions ship, each a top-level, name-keyed map — **not** nested under
each MCP server entry in the manifest — every one sharing the same glob
grammar: a plain pattern allows, a `!`-prefixed pattern denies, and the `"*"`
key is rename-proof (it constrains every server regardless of what a manifest
calls it, so a repo can't dodge a machine rule by renaming a server):

- `[policy.tools]` — per-server tool allow/deny globs (`policy.tool_allowed`).
- `[policy.egress]` — per-server outbound host globs, optionally port-scoped
  as `host:port` (`api.example.com:443`); a bare host means any port. The
  runtime proxy enforces the exact CONNECT port (`policy.egress_allowed` /
  `CompiledRuleset::egress_decision`).
- `[policy.secrets]` — per-server `${REF}` name globs (`policy.secret_allowed`).
- `[policy.filesystem]` — bundle-global `read`/`write` path globs (`FsPolicy`;
  no per-server split — a sandbox mount is per-run, not per-server).

Tools, egress, and secrets are **allow-by-default**: an absent key constrains
nothing. Filesystem writes are the deliberate exception on sandboxed paths:
an absent effective write scope leaves the workspace read-only, as described
below. Least privilege for the other dimensions is an explicit machine opt-in
— e.g. `[policy.tools] * = ["!*"]` to deny everything unless a bundle's own
allowlist narrows further. (No approval/confirm channel exists yet;
a future "confirm before calling" tier is unbuilt work, not a shipped
dimension.)

`compile(machine, bundle, servers)` folds both layers into a
`CompiledRuleset` — the canonical, serializable artifact every enforcer
consumes. It is lossless (each layer's allowlist is kept as an independent
AND-bound, so `tool_decision`/`egress_decision`/`secret_decision` can still
say *which* layer blocked a call) and rename-proof by construction (`"*"`
folds into every named server plus an `any` bucket for unknown names). The
in-process gateway consumes it for tool and secret decisions, while sandboxed
runs serialize the same policy semantics into the enforcing egress proxy and
runtime boundary. Keeping the artifact independent lets an enforcer change
without rewriting the policy engine. **The compiled ruleset is deliberately
not part of the trust digest**: one of its two inputs (machine policy) lives
outside the pinned bundle by design, so folding it into the digest would
create a second, machine-varying source of trust truth.

Enforcement honesty, per dimension (today) — the authoritative,
mode-by-dimension breakdown lives in [`ENFORCEMENT.md`](ENFORCEMENT.md); this is
the policy-engine summary:
- **Tools** — enforced: the gateway checks every call before dispatch
  (Layer 4's single enforcement point).
- **Secrets** — enforced, fail-closed: a denied `${REF}` never resolves,
  at both adapter render and the gateway's per-server resolver.
- **Egress** — enforced in sandbox mode: the egress proxy (host-process or
  lockdown sidecar, Layer 4) filters in-flight traffic against the compiled
  ruleset, Docker-verified end to end, and matches the exact CONNECT
  **host:port** (`[policy.egress]` supports `host:port` patterns, RULESET_VERSION
  2; see the dimension paragraph above). In host mode it is write/spawn-time
  only: a server's *declared* URL host is checked (render and gateway
  upstream construction), and a host hidden behind an unresolved `${REF}`
  fails closed if the server is constrained at all. The remaining host-mode
  gap: the write/spawn-time check matches only the declared *host* and defers
  the port to runtime, so an HTTPS-only intent isn't verifiable until the CLI
  actually connects.
- **Filesystem** — write scope enforced in sandbox mode: the workspace mounts
  read-only unless the effective write scope covers the workspace root
  (deny-by-default — the one dimension where absence means deny, because a
  sandbox grants nothing the policy doesn't name; a partial scope like
  `src/**` rounds DOWN to read-only, since the workspace is one all-or-nothing
  mount). The kernel enforces the `:ro` bind, not the harness. The semantics
  live in one place, `CompiledRuleset::workspace_write_decision`. Read scopes
  are informational while the only mount is the whole workspace, and host
  mode enforces neither — never present those as enforced.

Invariant (property-tested): for all bundle policies B and machine policies M,
`effective(B, M) ⊆ M`, across every dimension. This test is never deleted or
weakened.

## Layer 4 — Runtime (`crates/adapters`, `crates/runtime`, `crates/egress`)

**Adapters** compile a bundle into native config for each supported agent CLI
(Claude Code, Cursor, Codex, …). Normal rendering is one-way and
non-destructive; explicit `init`, `adopt`, and owned-server workflows can read
native state back into the manifest. The 13 adapters are data-driven YAML
descriptors, and writes stay blocked while any `${REF}` is unresolved.
Resolution completes *before* the compiler runs: render receives
a concrete server and a resolver, never a library or store to consult — which
is what lets a sandbox runtime materialize configs from core + adapters
alone. One trust note, stated plainly: user drop-in adapter descriptors
(`~/.agentstack/adapters/`) are part of the trusted computing base — they
alter how configs render and are trusted *because the user placed them*,
unlike bundle content, which is hostile. Inside a container that dir is
simply absent, which is expected and correct.

The four runtime modes (host, gateway, sandbox, lockdown) enforce different
dimensions to different depths; [`ENFORCEMENT.md`](ENFORCEMENT.md) is the
authoritative per-cell matrix. This section describes the mechanisms behind it.

**Host mode:** adapters write configs onto the bare machine. Honest framing:
advisory enforcement — static apply is user-invoked rather than trust-gated,
while render-time policy and fail-closed secret checks govern what gets
written. A CLI on the host can still bypass that config and could in principle
tamper with the trust store itself (Layer 2). Per dimension, host mode enforces
only secrets (fail-closed at the write boundary); tools, filesystem, and
audit are unsupported on this path, and egress is a coarse write-time host
check — see [`ENFORCEMENT.md`](ENFORCEMENT.md).

**Single enforcement point (declared, not just observed):** every MCP tool
call agentstack itself brokers — the gateway serve loop, the `agentstack mcp`
bridge, code mode — dispatches through one function, `Gateway::try_call`,
which consults the policy engine before any upstream I/O; the upstream
transport is private to it, so no other module *can* reach a server directly.
Any new brokered path must route through it — adding a second dispatch path
is a security-review event, not a refactor. (Rendered-config modes hand the
transport to the harness itself and are governed at write time — the
advisory framing above.)

**One enforcement-plan boundary for a sandbox run:** `run --sandbox` assembles
its security model in exactly one seam, `ExecutionPlan::build` — it checks
trust, compiles the effective (machine ∩ bundle) policy, resolves the mounts +
command, and picks the egress mode, returning one immutable plan. A command then
`execute`s that plan (which creates the fail-closed run log and the per-run proxy
token once, then dispatches to the mode) or `display`s it (`--plan`: a
Docker-free dry run that names the trust state, mode, and exact command). The
mode executors no longer re-derive any of it, so a run can't skip a check by
taking a different path — the same discipline as the single gateway dispatch,
applied to run assembly.

**Sandbox mode:** `agentstack run --sandbox` launches the CLI in a container
via the Docker API (`bollard`). Its configured HTTP(S) traffic is pointed at
the **egress proxy**, which enforces the compiled ruleset and emits one event
per decision (allow/block, host, server, tool). Two confinement strengths ship:

- **`--sandbox`** (host-process proxy): the container gets an ordinary bridge
  network and its `HTTPS_PROXY` points at a proxy on the host
  (`host.docker.internal`). This enforces the agent's *configured* egress and
  gates anything reachable only via the proxy — but a container that ignored
  the proxy env could still reach the open internet directly. The listener
  necessarily binds a broad address so the container can reach it, so the
  peer is authenticated: a per-run random token rides in the proxy URL's
  userinfo and the proxy 407s any CONNECT that doesn't present it — the
  token, not the bind address, is what stops a LAN neighbor from using the
  proxy as an open relay (and the same token authenticates the sandbox to
  the lockdown sidecar).
- **`--lockdown`** (no direct route): the container is attached ONLY to an
  internal Docker network — no host route, no internet, no DNS beyond it —
  whose single reachable peer is the **egress-proxy sidecar container**
  (`docker/egress-proxy.Dockerfile`, the `egress` crate's binary). The sidecar
  is dual-homed onto a second ordinary network so it (and only it) forwards
  allowed traffic out. Ignoring the proxy env then reaches nothing: the
  confinement is topological, not convention. The ruleset crosses the process
  boundary as a serialized `CompiledRuleset` the sidecar fails closed on if
  its version is newer than the binary understands. Both modes are
  Docker-verified end to end through the real binary (`sandbox_egress`,
  `sandbox_cli_e2e`, `sandbox_fs`, `sandbox_lockdown`, `sidecar_image`).

The egress proxy is the hardest engineering in the system — harder than the
async learning curve. Known-hard sub-problems, stated up front:
- **Per-server attribution**: attributing egress to a specific MCP server
  requires one proxy identity per server (distinct ports, containers, or
  credentials), not one shared funnel.
- **HTTPS filtering** (enforced): the proxy decides on the CONNECT authority
  and, once TLS starts, requires the ClientHello's SNI to match that host —
  so a client can't tunnel to an allowed front and then ask for a denied host
  behind it (domain fronting). No TLS interception/MITM. Hostnames are
  normalized (lowercase, trailing dot stripped) before matching so casing
  can't dodge a deny.
- **Anti-SSRF** (enforced): an allowed *name* can still resolve to the host's
  own network. The proxy resolves once and requires every resolved address to
  be global unicast — loopback, private, link-local (incl. the
  `169.254.169.254` metadata IP), unique-local, and reserved ranges are
  refused — then dials the validated address (no second resolution, closing
  DNS rebinding). Literal-IP CONNECTs flow through the same check. Tests/demos
  that dial the host gateway opt out via `AGENTSTACK_ALLOW_LOCAL_TARGETS`;
  production never sets it.
- **DNS** is itself an exfiltration channel and needs to be routed and
  filtered, not left open — the container resolves nothing directly; the proxy
  resolves only allowed names.
- **Peer authentication** (enforced): the listener must bind a broad address so
  the container can reach it, so a per-run token — minted by the CLI, injected
  as the sandbox's `HTTPS_PROXY` credentials and into the sidecar's env — is
  what authenticates the peer, not the bind. A CONNECT without valid
  `Proxy-Authorization` gets a 407 and is recorded, so the proxy can't be used
  as an open relay by anything else that can route to it.

**Scope honesty — exfiltration through allowed channels:** even a perfectly
enforced allowlist permits traffic to allowed hosts, including the model API
itself — a prompt-injected agent can leak data through any host the policy
allows. AgentStack's claim is *untrusted declarations are not auto-activated
and unapproved egress is blocked on the enforced paths* — not that
exfiltration is impossible.

(The shipped `agentstack proxy` token-observation relay is unrelated to this
crate and keeps its name; the enforcement crate is `egress`.)

Design references (not dependencies): Sandcastle's provider model, branch
strategy, and event hooks are good prior art for orchestration shape.

## Layer 5 — Flight recorder (`crates/recorder`)

Append-only, per-run JSONL records execution start/finish/limits, sandbox
lifecycle, egress decisions, brokered tool calls (with argument digests), and
secret-reference access (reference names, never values). `agentstack report
<run>` renders that evidence for humans or JSON consumers and can supplement
older runs from the separate global call audit log.

Trust-store mutations and per-run token/cost events are not yet wired. The
JSONL is append-only by convention, not cryptographically tamper-evident.

Scope discipline: a log with a good viewer, not an observability platform.

## Layer 6 — Registry (future, Phase 4)

Push/pull of signed bundles. The trust gate verifies signatures against
publisher keys; content-pinning and review flow are inherited unchanged.
Starts life as a curated Git repository of signed bundles — no infrastructure
until demand proves it.

## Crate dependency rules

Exact internal edges (anything not listed is forbidden):

```
core     → (nothing)
trust    → core
policy   → core
recorder → core
adapters → core
runtime  → core, policy, recorder
egress   → core, policy, recorder
executor → core, runtime, recorder
cli      → everything
```

The experimental execution boundary is specified in the
[`tools_execute` threat model](design/tools-execute-threat-model.md) and
[runtime/ownership ADR](design/adr-tools-execute-runtime.md). Its user-visible
claims are intentionally narrower than this architecture description and live
in [`ENFORCEMENT.md`](ENFORCEMENT.md#experimental-tools_execute).

`adapters` deliberately does **not** depend on `policy`: the fail-closed secret
check happens *before* render, in the caller. Re-granting that edge is a
deliberate architecture change, not a Cargo.toml edit. (This edge was once
listed and withdrawn — see [`HISTORY.md`](HISTORY.md).)

`core` depends on nothing internal; nothing depends on `cli`. `trust` and
`policy` are the security-critical crates: they depend on `core` only, stay
as small as possible, carry `#![forbid(unsafe_code)]`, keep the restricted
dependency list (see CLAUDE.md rule 6), and their property-tested invariants
are human-reviewed line by line.
