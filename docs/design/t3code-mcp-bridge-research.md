# t3code MCP harness bridge — research record

> **Status:** research complete, bridge **not** implemented (fails the
> suitability gate; decision below)
>
> **Scope:** the first checkbox of `TODO.md` §"t3code MCP harness bridge —
> research only": inventory the real t3code surfaces and decide whether a
> governed child-launch backend is buildable on them today.
>
> **Inspected:** t3code @ `agentstack-panel` (ff1df0cca), 2026-07-23.

## What t3code actually exposes

t3code has three distinct surfaces. Only one launches anything, and none is a
child-launch contract.

### 1. The `/mcp` endpoint is browser-preview only

- Streamable-HTTP MCP server mounted on t3code's own HTTP port at `/mcp`
  (`apps/server/src/mcp/McpHttpServer.ts`).
- Exactly 13 tools, all `preview_*` (open/navigate/click/type/evaluate/
  snapshot/record a collaborative browser tab). No tool creates, lists,
  stops, or observes a coding-CLI session.
- Auth is a per-thread bearer token minted by t3code itself
  (`McpSessionRegistry`) with a `preview`-only capability set, ≤ 8 h lifetime,
  hash-at-rest. t3code injects the endpoint + token into the config of the
  coding CLI *it is already launching* (per-adapter injection, e.g.
  `ClaudeAdapter` passes `mcpServers["t3-code"]`). It is inward-facing
  plumbing: an external process cannot use it to cause or observe a launch.

### 2. The `/ws` orchestration protocol launches sessions — chat-shaped

The WebSocket RPC (`apps/server/src/ws.ts`, contracts in
`packages/contracts/src/orchestration.ts` / `provider.ts` /
`providerRuntime.ts`) can create a thread, start/stop a provider session,
send/interrupt a turn, and stream a rich typed event taxonomy with semantic
completion (`turn.completed` state, `session.exited` exitKind). Identities
are logical (`ThreadId`, `TurnId`, `ProviderInstanceId`).

What it cannot provide, measured against the workflow child-run contract
(`docs/design/workflows-capability.md` §8):

| Child-run requirement | t3code today |
| --- | --- |
| Launch an admitted, frozen argv/config | Binary path + launch flags are server-side settings per pre-provisioned `ProviderInstanceId`; no per-call argv or config injection |
| Stable child identity | Logical thread/turn ids only; no PID or process handle on session contracts |
| Process-level result | Semantic turn/session states only; no exit code/signal |
| Cancellation | Turn interrupt + session stop exist (semantic, adequate) |
| Evidence linkage | Rich event stream exists, but no per-session process identity to bind it to a `RunEnvelope` |
| Capability negotiation | No protocol-version handshake; compatibility is implicit via the shared contracts package |

### 3. The AgentStack integration points the other way

t3code's `AgentstackCli` service shells out to the agentstack binary with a
closed action enum — t3code consuming AgentStack, deliberately without a
client-supplied command line. It is not a launch surface for us.

## Decision

**Do not build the MCP workflow bridge now.** The existing surface fails the
suitability gate on four of the six required properties (argv/config
admission, process identity, process-level result, version negotiation).
Adapting to what does exist would mean AgentStack submitting prompts to
t3code's pre-configured provider instances — that launches whatever the
t3code server settings say, not the frozen `ExecutionPlan` the workflow
engine admitted, and would create exactly the second launch path with
unverifiable authority that the non-negotiables forbid.

Direct CLI child launch (`run --locked` composition) remains the only spawn
path, as the reference implementation and fallback.

## What would change the answer

A future t3code contract offering, behind its authenticated `/ws` (or an
equivalent negotiated channel):

1. a session-start input accepting an opaque, pre-admitted launch reference
   (or a fixed argv+config supplied by the caller) rather than a
   server-settings instance;
2. a stable child identity bound to the OS process, with process-level exit
   status;
3. an explicit protocol-version/capability handshake that fails closed;
4. cancellation and event streams addressable by that child identity.

If those land upstream, the remaining TODO research items (mapping to the
child-run contract, the narrow backend, the no-bypass witnesses) become
implementable behind the existing governed child-launch seam. Until then the
lane stays closed.
