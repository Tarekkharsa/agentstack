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

A bundle arrives (cloned, pulled, copied) and is inert by construction. `agentstack review` renders a human-readable diff of everything the bundle declares. Trusting it pins the lockfile digest into the machine-local trust store. At run time, the policy engine intersects the bundle's requested policy with the machine's rules, compiles a ruleset, and hands it to the runtime. In sandbox mode the agent CLI runs in a container whose only network route is the egress proxy enforcing that ruleset. Every tool call, block, secret resolution, and cost figure streams into an append-only run log.

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
      egress: ["api.search.example"]

policy:
  filesystem:
    read: ["./workspace"]
    write: ["./workspace/out"]
  tools:
    confirm: ["*_delete", "*_send"]

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

**Honest limitation:** the trust store and machine policy live under
`~/.agentstack/`, which is writable by the user — and in host mode the agent
CLI runs *as* the user, so a compromised agent could modify them and
self-trust a bundle. Only sandbox mode removes this. As mitigation, the
recorder logs every trust-store mutation as tamper evidence.

This layer must work standalone — valuable with no sandbox, no registry.

## Layer 3 — Policy engine (`crates/policy`)

Two inputs: the bundle's requested policy and the machine policy
(`~/.agentstack/policy.yaml`). The machine policy lives outside every repo's
tree, so no repo content can alter it — but see the host-mode limitation in
Layer 2: it is still a user-writable file.

Output: effective policy = **intersection**. Bundles can narrow, never widen.
(The shipped machine-first `[policy.tools]` check is the v0 of this rule;
Phase 1 generalizes it into a real intersection engine.)

Policy dimensions: network egress per MCP server (allowlist of hosts),
filesystem read/write scopes, tool allow/deny/confirm lists, secret access
per server.

The compiled ruleset is a plain serializable artifact handed to the egress
proxy — this clean interface is what lets the proxy be rewritten (e.g. in a
separate process or language) without touching the engine.

Invariant (property-tested): for all bundle policies B and machine policies M,
`effective(B, M) ⊆ M`. This test is never deleted or weakened.

## Layer 4 — Runtime (`crates/adapters`, `crates/runtime`, `crates/egress`)

**Adapters** are one-way compilers from bundle → native config for each
supported agent CLI (Claude Code, Cursor, Codex, …). Generated fresh per run,
never hand-edited, never read back. The 13 data-driven YAML adapters shipped
in v0.8.x move here as-is; writes stay blocked while any `${REF}` is
unresolved.

**Host mode** (Phase 1): adapters write configs onto the bare machine.
Honest framing: advisory enforcement — the trust gate governs what gets
written, but a CLI on the host could bypass policy, and could in principle
tamper with the trust store itself (Layer 2).

**Sandbox mode** (Phase 2): `agentstack run --sandbox` launches the CLI in a
container via the Docker API (`bollard`). The container has no direct network;
its only route out is the **egress proxy**, which enforces the compiled
ruleset and emits one event per decision (allow/block, host, server, tool).
The container boundary is what upgrades policy from advisory to enforced.

The egress proxy is the hardest engineering in the system — harder than the
async learning curve. Known-hard sub-problems, stated up front:
- **Per-server attribution**: attributing egress to a specific MCP server
  requires one proxy identity per server (distinct ports, containers, or
  credentials), not one shared funnel.
- **HTTPS filtering**: host allowlisting means CONNECT/SNI-based filtering —
  no TLS interception/MITM.
- **DNS** is itself an exfiltration channel and needs to be routed and
  filtered, not left open.

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
adapters → core, policy
runtime  → core, policy, recorder
egress   → core, policy, recorder
cli      → everything
```

`core` depends on nothing internal; nothing depends on `cli`. `trust` and
`policy` are the security-critical crates: they depend on `core` only, stay
as small as possible, carry `#![forbid(unsafe_code)]`, keep the restricted
dependency list (see CLAUDE.md rule 6), and their property-tested invariants
are human-reviewed line by line.
