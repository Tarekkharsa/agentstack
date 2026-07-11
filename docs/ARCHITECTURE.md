# AgentStack — Architecture

## Vision

AgentStack packages, runs, and governs AI agents — skills, tools, and MCP servers — as trusted, portable bundles.

The strategic frame: the **agent bundle** is the standard unit (the way the image was Docker's unit). Everything else in the system gates it, constrains it, records it, or distributes it. Config unification across agent CLIs is the adoption wedge; the trust gate, firewall, and audit trail are the durable value. The registry/marketplace is the endgame, only viable because trust and signing exist first.

Core principle: **nothing runs until it's trusted, and nothing trusted runs unobserved.**

## Where this starts

This is not a greenfield design. The shipped v0.8.x binary already implements v0 of most layers: manifest + lockfile digests, a trust gate that pins the manifest and lockfile and re-gates on edit, a machine-first tool policy no repo can loosen, a call audit log, fail-closed secret resolution, and 13 data-driven CLI adapters. This document describes the target shape that code is extracted and hardened into — the deltas are called out per layer.

## The flow

```
bundle (inert) → trust gate → policy engine → sandboxed runtime → flight recorder
                                    ↑                  ↑
                             machine rules      secrets resolver
```

A bundle arrives (cloned, pulled, copied) and is inert by construction. `agentstack review` renders a human-readable diff of everything the bundle declares. Trusting it pins the lockfile digest into the machine-local trust store. At run time, the policy engine intersects the bundle's requested policy with the machine's rules, compiles a ruleset, and hands it to the runtime. In sandbox mode the agent CLI runs in a container routed through an egress proxy enforcing that ruleset — and in lockdown mode the proxy sidecar is topologically the *only* route out. Every tool call, block, secret resolution, and cost figure streams into an append-only run log.

## Layer 1 — The bundle (`crates/core`)

A bundle is a directory. It is **declarative and inert**: pure data, nothing executes.

```
my-agent/
  agent.yaml          # manifest
  agent.lock          # content digests of everything referenced
  instructions/       # system prompt / instruction files
  skills/             # skill files (treated as untrusted input)
```

`agent.yaml` (sketch — this shows the required *semantics*; the concrete format starts from the shipped `agentstack.toml` and is settled during Phase 1, with breaking changes free):

```yaml
name: research-agent
version: 0.3.0

instructions:
  - instructions/main.md

skills:
  - skills/summarize.md
  - skills/cite-sources.md

mcp:
  - name: web-search
    command: ["npx", "-y", "@example/search-mcp"]
    env:
      SEARCH_API_KEY: ${SEARCH_API_KEY}     # secret ref, never a literal

policy:
  tools:
    web-search: ["!*_delete"]
  egress:
    web-search: ["api.search.example"]
  secrets:
    web-search: ["SEARCH_API_KEY"]
  filesystem:
    read: ["./workspace"]
    write: ["./workspace/out"]

runtime:
  clis: [claude-code, cursor]               # adapters to materialize
```

`agent.lock` pins every referenced file and every MCP definition to a SHA-256
digest, plus a digest of the manifest itself. The lockfile digest is the
bundle's identity fingerprint. An optional detached signature
(ed25519 over the lockfile) enables registry distribution later.

Key decisions:
- Skills are content-pinned like code, because skill files are a
  prompt-injection delivery mechanism. Reviewing a bundle means reviewing
  skill *content*, not just the file list. (Delta from v0.8.x, which pins
  the manifest and lockfile but not the content of referenced files —
  closing that gap is Phase 1 work.)
- Secrets appear only as `${REF}` placeholders, resolved by the OS keychain
  (`keyring`) or varlock. Resolution happens in memory at run time.
  Unresolvable secret → fail closed.

## Layer 2 — Trust gate (`crates/trust`)

Machine-local trust store: `bundle identity → trusted lockfile digest`
(plus who trusted it, when, and optionally the publisher key).

States: **untrusted** (default for anything new or changed) → **reviewed** →
**trusted**. Untrusted means: no MCP spawn, no skill enters context, no secret
resolves, no adapter output is written.

`agentstack review` shows the diff since the last trusted digest: manifest
changes, skill content changes, MCP definition changes, policy changes.
Trust binds to the digest, so any byte change anywhere re-gates automatically.

Invariant (property-tested): flipping any single byte in any pinned file
produces a digest mismatch and an untrusted state.

**Principle — trust is a portable claim about content, never a fact about a
machine.** A trust decision asserts "this exact content (this digest, and in
Phase 4 this signature) was reviewed and approved" — it must carry no
assumption about *where* that approval happened or where it will be honored.
The party that trusts a bundle and the machine that runs it may be different
parties: a maintainer signs, CI verifies, a runtime executes, an auditor
reads the record. No code may couple trust to local identity — machine
hostnames, local usernames, or absolute local paths must never become part
of what a trust claim *means*. (Shipped delta: the v0 store keys entries by
canonicalized local project path, which is fine as a lookup key on one
machine but is not the claim itself; the claim stays digest-shaped.)

**Honest limitation:** the trust store and machine policy live under
`~/.agentstack/`, which is writable by the user — and in host mode the agent
CLI runs *as* the user, so a compromised agent could modify them and
self-trust a bundle. Only sandbox mode removes this. As mitigation, the
recorder logs every trust-store mutation as tamper evidence.

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
- `[policy.egress]` — per-server outbound host globs (`policy.egress_allowed`).
- `[policy.secrets]` — per-server `${REF}` name globs (`policy.secret_allowed`).
- `[policy.filesystem]` — bundle-global `read`/`write` path globs (`FsPolicy`;
  no per-server split — a sandbox mount is per-run, not per-server).

All four are **uniform allow-by-default**: an absent key constrains nothing.
Least privilege is an explicit machine opt-in, not a per-dimension special
case — e.g. `[policy.tools] * = ["!*"]` to deny everything unless a bundle's
own allowlist narrows further. (No approval/confirm channel exists yet;
a future "confirm before calling" tier is unbuilt work, not a shipped
dimension.)

`compile(machine, bundle, servers)` folds both layers into a
`CompiledRuleset` — the canonical, serializable artifact every enforcer
consumes. It is lossless (each layer's allowlist is kept as an independent
AND-bound, so `tool_decision`/`egress_decision`/`secret_decision` can still
say *which* layer blocked a call) and rename-proof by construction (`"*"`
folds into every named server plus an `any` bucket for unknown names). The
in-process gateway consumes it today; the Phase-2 egress proxy and sandbox
runtime are meant to receive the identical artifact serialized across the
process boundary — this is the clean interface that lets the proxy be
rewritten without touching the engine. **The compiled ruleset is deliberately
not part of the trust digest**: one of its two inputs (machine policy) lives
outside the pinned bundle by design, so folding it into the digest would
create a second, machine-varying source of trust truth.

Enforcement honesty, per dimension (today):
- **Tools** — enforced: the gateway checks every call before dispatch
  (Layer 4's single enforcement point).
- **Secrets** — enforced, fail-closed: a denied `${REF}` never resolves,
  at both adapter render and the gateway's per-server resolver.
- **Egress** — enforced in sandbox mode: the egress proxy (host-process or
  lockdown sidecar, Layer 4) filters in-flight traffic against the compiled
  ruleset, Docker-verified end to end. In host mode it is write/spawn-time
  only: a server's *declared* URL host is checked (render and gateway
  upstream construction), and a host hidden behind an unresolved `${REF}`
  fails closed if the server is constrained at all. One known gap either
  way: the decision matches the *host*, not the port — an allowed host is
  reachable on any port, and an HTTPS-only intent is not yet expressible.
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

**Adapters** are one-way compilers from bundle → native config for each
supported agent CLI (Claude Code, Cursor, Codex, …). Generated fresh per run,
never hand-edited, never read back. The 13 data-driven YAML adapters shipped
in v0.8.x move here as-is; writes stay blocked while any `${REF}` is
unresolved. Resolution completes *before* the compiler runs: render receives
a concrete server and a resolver, never a library or store to consult — which
is what lets a sandbox runtime materialize configs from core + adapters
alone. One trust note, stated plainly: user drop-in adapter descriptors
(`~/.agentstack/adapters/`) are part of the trusted computing base — they
alter how configs render and are trusted *because the user placed them*,
unlike bundle content, which is hostile. Inside a container that dir is
simply absent, which is expected and correct.

**Host mode** (Phase 1): adapters write configs onto the bare machine.
Honest framing: advisory enforcement — the trust gate governs what gets
written, but a CLI on the host could bypass policy, and could in principle
tamper with the trust store itself (Layer 2).

**Single enforcement point (declared, not just observed):** every MCP tool
call agentstack itself brokers — the gateway serve loop, the `agentstack mcp`
bridge, code mode — dispatches through one function, `Gateway::try_call`,
which consults the policy engine before any upstream I/O; the upstream
transport is private to it, so no other module *can* reach a server directly.
Any new brokered path must route through it — adding a second dispatch path
is a security-review event, not a refactor. (Rendered-config modes hand the
transport to the harness itself and are governed at write time — the
advisory framing above.)

**Sandbox mode** (Phase 2): `agentstack run --sandbox` launches the CLI in a
container via the Docker API (`bollard`). Its only route out is the **egress
proxy**, which enforces the compiled ruleset and emits one event per decision
(allow/block, host, server, tool). The container boundary is what upgrades
policy from advisory to enforced. Two confinement strengths ship:

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
allows. AgentStack's claim is *untrusted code stays inert and unapproved
egress is blocked* — not that exfiltration is impossible.

(The shipped `agentstack proxy` token-observation relay is unrelated to this
crate and keeps its name; the enforcement crate is `egress`.)

Design references (not dependencies): Sandcastle's provider model, branch
strategy, and event hooks are good prior art for orchestration shape.

## Layer 5 — Flight recorder (`crates/recorder`)

Append-only, per-run JSONL log fed by egress-proxy events, adapter events, and
the CLI output stream: every tool call with arguments, every policy block,
every secret resolution (which ref, never the value), every trust-store
mutation, token/cost, wall time. `agentstack report <run>` renders a
human-readable run report.

The shipped call audit log (`~/.agentstack/audit/calls.jsonl`) is the v0 of
this layer and seeds the event types.

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
cli      → everything
```

(The 2026-07-11 security review flagged that this table once granted
`adapters → policy` while the crate never used it — the fail-closed secret
check happens *before* render, in the caller. The edge is withdrawn to match
reality; re-granting it is a deliberate architecture change, not a Cargo.toml
edit.)

`core` depends on nothing internal; nothing depends on `cli`. `trust` and
`policy` are the security-critical crates: they depend on `core` only, stay
as small as possible, carry `#![forbid(unsafe_code)]`, keep the restricted
dependency list (see CLAUDE.md rule 6), and their property-tested invariants
are human-reviewed line by line.
