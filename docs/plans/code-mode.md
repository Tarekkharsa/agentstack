# AgentStack Plan: Compact Tool Surface + Code Mode

Date: 2026-06-30

Status: proposal (design notes, not yet implemented)

Companion to [`portable-agent-runtime-vision.md`](./portable-agent-runtime-vision.md)
and [`central-store.md`](./central-store.md) (the central capability library the
dynamic search/execute surface loads from). This plan extends the existing
runtime gateway; it does not replace the compiler. Read the North Star there
first.

## TL;DR

Today `agentstack mcp` exposes ~16 control-plane tools **plus every tool of
every proxied upstream server** (`src/gateway.rs` namespaces them all into
`tools/list`). As a user adds servers, the agent's context fills with tool
definitions it may never call — the exact bloat that
[Anthropic's "code execution with MCP"](https://www.anthropic.com/engineering/code-execution-with-mcp)
and Cloudflare's Code Mode were built to remove.

This plan collapses the **proxied** surface behind compact discovery plus
generated bindings — `tools_search` and `tools_bindings` — while leaving
agentstack's own control-plane tools and its compiler identity untouched.

The execution model is deliberately chosen to fit our vision: **agentstack does
not embed a sandbox.** It emits typed bindings and lets the harness's own code
runtime execute them. agentstack stays a compiler, not a runtime daemon.

## Why this fits the vision (and where it would break it)

From `portable-agent-runtime-vision.md` — Product Principles:

- "Prefer reproducibility over magic."
- "Prefer explicit previews over silent mutation."
- "Do not become a marketplace before becoming a trusted runtime manager."

And the trust gate already enforced in `src/mcp_server.rs` (D20): the agent
proposes to the **manifest only**; a human runs `apply`. The gateway does proxy
live MCP tool calls to declared upstreams, but **no arbitrary code is executed
by agentstack, and config writes remain human-gated.**

Two ways to deliver code mode:

1. **agentstack embeds a JS/V8 sandbox and runs agent-written code itself.**
   This is what kody does (it is a Cloudflare Workers app that *owns* a V8
   isolate runtime). For us this would turn agentstack from an
   occasionally-run compiler into an always-on runtime in the hot path of
   every tool call, with a new arbitrary-code-execution security surface. It
   contradicts "do not become a runtime before you are a trusted manager."

2. **agentstack emits typed bindings; the harness executes them.** Claude Code,
   Codex, and friends already have a code-execution/bash tool and a sandbox.
   agentstack is uniquely positioned *next to* a capable harness (kody is not —
   it serves sandbox-less hosts like ChatGPT, so it had no choice but to be the
   sandbox). We generate a small typed client for the proxied servers; the
   harness runs it in its own sandbox; agentstack only resolves secrets and
   proxies the actual MCP calls.

**This plan commits to option 2.** Option 1 is recorded as a future fork
(§ "Future fork: hosted execute") to be taken only if agentstack must serve
sandbox-less MCP hosts directly.

## Reference: what TanStack AI proves

TanStack AI's Code Mode docs are the clearest framework-level statement of the
pattern:

- **True Code Mode is an executor.** TanStack's `createCodeMode` creates an
  `execute_typescript` tool plus a matching system prompt. The model emits one
  TypeScript program; the program runs in a sandbox; only the final result comes
  back.
- **The sandbox is explicit.** TanStack requires an `IsolateDriver` and supports
  Node/V8 isolates, QuickJS WASM, and Cloudflare Workers. This confirms our
  boundary: if agentstack ships a real `tools_execute`, it owns a sandbox and a
  new runtime security surface.
- **Bindings are the bridge.** TanStack converts tools into typed function
  stubs (`external_*`) and exposes utilities like `toolsToBindings` and
  `generateTypeStubs`. That maps cleanly to our `tools_bindings` phase: generate
  typed call surfaces without executing arbitrary code ourselves.
- **Lazy tools match our Phase 1.** TanStack's lazy-tool flow keeps large tool
  definitions out of the initial prompt and exposes a discovery tool that
  returns descriptions and schemas on demand. That is the same pressure behind
  `tools_search`: progressive disclosure for large live tool catalogs.

So TanStack changes the framing slightly: Phase 1 is lazy discovery, Phase 2 is
typed bindings, and the Future Fork is the actual TanStack-style
`execute_typescript` equivalent.

## Reference: what kody proved

`kentcdodds/kody` is the cleanest existing implementation of this pattern. What
to copy and what to ignore:

- **Wire surface is 3 tools** (`packages/worker/src/mcp/register-tools.ts`):
  `search`, `execute`, `open-generated-ui`. Everything else is a capability
  behind code mode. Copy the discipline.
- **`search` shape** (`docs/use/search.md`): a `query` returns compact ranked
  markdown; a second call with `entity: "{id}:{type}"` returns full type
  definitions plus a ready-to-run snippet. No separate `detail` flag — one tool,
  two depths. **Copy this exactly.**
- **`execute` shape** (`docs/use/execute.md`): one ephemeral ES module,
  `import { codemode } from 'kody:runtime'`, call `codemode.capability(input)`,
  chain many calls in one turn. **Copy the *interface* (`codemode.x()`), not the
  runtime** — kody's runtime is a cloud V8 sandbox we will not build.
- **Ignore for now:** kody's `packages` primitive (saved repo-backed executable
  code), per-user cloud isolation, Workers/D1 storage. Our analog to "saved
  capability code" is skills/packs, which are config+prose, not executable
  modules. Do not conflate them.

## Scope boundaries (what this plan does NOT touch)

- **The compiler.** `apply`, adapters, render, packs, secrets — unchanged.
- **agentstack's 16 control-plane tools** (`agentstack_search`,
  `agentstack_add_from`, `agentstack_session_*`, `agentstack_explain`,
  `agentstack_diff`, …). These are a curated *human-trust* surface, not a
  capability graph. They stay as explicit tools. (Note: `agentstack_search`
  searches the *catalog/registry* for things to install; the new `search`
  searches *live proxied tools* to call. Different jobs — keep both, but see
  §Naming.)
- **stdio upstreams.** Gateway v1 is HTTP-only (`src/gateway.rs`). Code mode
  inherits that boundary in phase 1; stdio is a later gateway concern.

## Naming

To avoid colliding with the existing `agentstack_search` (catalog discovery),
the new tools are namespaced to the runtime surface:

- `tools_search` — find and inspect live, callable upstream tools.
- `tools_bindings` — generate the typed client the harness uses to call them.

The Phase 2 tool is **not** named `tools_execute`: in option 2 agentstack does
not execute the code (the harness does), so a tool named "execute" that refuses
to execute would misrepresent the contract. The name `tools_execute` is reserved
for the hosted-execution Future Fork, where agentstack *would* run the code.

(Final names TBD; the existing catalog tool keeps `agentstack_search`.)

## Current state (verified 2026-06-30)

- `src/gateway.rs` — `Gateway::from_manifest` builds HTTP upstreams from the
  manifest, resolving `${REF}`s at call time. `namespaced_tools()` lists every
  upstream tool (`<server>__<tool>`), capped at 600-char descriptions with a
  `[via <server>]` provenance prefix. `try_call(name, args)` forwards a
  namespaced call to the right upstream. Tools are cached after first list.
- `src/mcp_server.rs` — `tools/list` returns `tool_defs()` (our 16) **plus**
  `gateway.namespaced_tools()`. `tools/call` tries `gateway.try_call` first,
  then our own tools. This is where the bloat enters and where the fix lands.

So the plumbing for compact discovery and call-forwarding already exists: the
gateway can enumerate and call every upstream tool. We are changing *what the
agent sees*, not building a new upstream transport.

---

## Phase 1: Compact discovery (`tools_search`)

Goal: stop dumping the full proxied catalog into context. Make discovery a
ranked, two-depth lookup. **Highest value, lowest risk — do this first.**

### Behavior

- `tools/list` no longer appends `gateway.namespaced_tools()`. Instead it adds
  one tool, `tools_search` (and keeps the 16 control-plane tools).
- `tools_search({ query, limit?, maxResponseSize? })` → compact ranked markdown:
  one line per matching upstream tool with `server__tool`, a one-line summary,
  and the entity ref to inspect it.
- `tools_search({ entity: "server__tool:tool" })` → full detail for one tool:
  its `inputSchema`, the source server, provenance/safety note, and a
  ready-to-run code-mode snippet (a call against the generated client). No
  separate detail flag (kody pattern).
- Ranking v1: substring/token match over tool name + description + server name.
  Keep it boringly deterministic; no embeddings in v1.

### Implementation sketch

- `src/gateway.rs`:
  - Add `search(query, limit) -> Vec<Hit>` over the cached upstream tool list.
  - Add `describe(entity) -> Option<ToolDetail>` returning the upstream's raw
    `inputSchema` + provenance.
  - Keep `namespaced_tools()` (still used internally to build the index).
- `src/mcp_server.rs`:
  - Drop the `tools.extend(gateway.namespaced_tools())` line from `tools/list`.
  - Add `tools_search` to `tool_defs()` and route it in `run_tool`.

### Trust / safety

- The manifest stays the allowlist: search only ranks tools from servers the
  manifest declares. Tool-poisoning guard (description cap + provenance prefix)
  carries over to search cards.
- Read-only. No manifest writes, no execution.

### Exit criteria

- [ ] `tools/list` returns a bounded surface regardless of upstream count.
- [ ] `tools_search(query)` returns ranked compact cards with entity refs.
- [ ] `tools_search(entity)` returns one tool's input schema + call snippet.
- [ ] Golden tests: list shape is stable; search ranks a known fixture server.
- [ ] Token footprint of `tools/list` is independent of how many tools the
      upstreams expose (measured against a multi-tool mock).

---

## Phase 2: Typed bindings for harness-run code (`tools_bindings`)

Goal: let the agent write one small program that calls several upstream tools,
runs in **the harness's own sandbox**, and returns only the final result.

### The binding model

- agentstack generates a typed client module for the project's proxied servers,
  e.g. `.agentstack/codemode/client.ts` (and/or `.py`), where each upstream tool
  is a function:

  ```ts
  // generated — do not edit
  import { call } from "./agentstack-runtime"
  /** [via figma] Get a file's node tree. */
  export const figma = {
    get_file: (input: { fileKey: string }) => call("figma__get_file", input),
  }
  ```

- `./agentstack-runtime` is a thin shim that POSTs `{ name, arguments }` to the
  local gateway endpoint (see §Transport). agentstack resolves secrets and
  forwards to the real upstream via the existing `try_call` path.
- The agent imports the client, writes orchestration code, and runs it with the
  **harness's existing code/bash tool**. agentstack does not execute it.

This is the key vision-preserving move: arbitrary code runs in the harness
sandbox the user already trusts; agentstack remains a config compiler that also
brokers MCP calls. No embedded interpreter, no new daemon-owned exec surface.

### Transport for runtime calls

The shim needs a local endpoint to reach the gateway. Options (decide in design
review):

1. **stdio bridge** — the harness already spawns `agentstack mcp`; expose a
   call-forwarding endpoint the shim uses. Simplest if the harness keeps one
   connection.
2. **loopback HTTP** — `agentstack mcp` opens a `127.0.0.1` token-gated socket
   (mirrors the dashboard server in `src/dashboard/server/`) the shim POSTs to.
   Cleaner for arbitrary child processes; reuses the dashboard's localhost+token
   pattern.

Recommendation: start with loopback HTTP, token-gated, project-scoped — it
matches an existing trusted pattern and decouples the shim from the MCP stdio
lifecycle.

### `tools_bindings` tool

`tools_bindings` is a **generator, not an executor** — the name says exactly what
it does. It returns the typed client snippet for the proxied servers plus a short
recipe for the harness to run, and regenerates the client if the manifest
changed. It never runs the agent's code; the harness's own code tool does.

This is the deliberate split flagged in §Naming: phase 2 ships honest code
*generation*; server-side code *execution* is the Future Fork below, and only
that fork earns the name `tools_execute`.

### Generation lifecycle

- `agentstack apply` (or a new `agentstack codemode --write`) regenerates the
  client when servers change. Generated files are agentstack-owned, contained,
  and pruned like other rendered artifacts (reuse the ownership/containment
  rules from the pack work).
- Secrets never appear in generated bindings — the shim resolves `${REF}`s at
  call time via the gateway, exactly as `from_manifest` does now.

### Exit criteria

- [ ] `agentstack` generates a typed client for proxied servers, secret-free.
- [ ] A loopback (or stdio) runtime endpoint forwards shim calls through the
      existing gateway `try_call` path with per-call secret resolution.
- [ ] A documented recipe: "discover with `tools_search`, write a module against
      the generated client, run it with your harness's code tool."
- [ ] Generated files are owned/contained/pruned; never committed with secrets.
- [ ] Tests: generation is deterministic; shim call round-trips to a mock
      upstream; missing-secret blocks the call with a clear message.

---

## Phase 3: Polish, safety, observability

- [ ] `agentstack explain` covers code-mode bindings: which servers, which
      secrets, which files generated, network egress per server.
- [ ] Dashboard surfaces the active proxied surface and per-call provenance.
- [ ] Per-server enable/disable for the runtime surface via profiles (a profile
      already fences skills; extend the same fence to proxied tools).
- [ ] Optional: rank quality (synonyms/tags) if substring search proves thin.
- [ ] Document the boundary clearly: code mode is for *proxied upstream tools*;
      it is not a way to mutate agentstack config (that stays human-gated).

---

## Future fork: hosted `execute` (only if we serve sandbox-less hosts)

If agentstack must serve MCP hosts that have **no** code runtime of their own
(e.g. ChatGPT), the bindings model is not enough — agentstack would have to run
the code itself, like kody. That means embedding a JS engine (deno_core /
QuickJS / boa) or shelling to a locked-down node/deno, with isolation, resource
limits, and an arbitrary-code-execution threat model.

This is a **separate product bet** that turns agentstack into a runtime. Do not
start it until:

1. Phases 1–2 ship and the compact surface is proven valuable next to a real
   harness, and
2. there is concrete demand to drive a sandbox-less host.

Record it here so the option is explicit, not accidental.

## Decisions to confirm before building

1. Final tool names (`tools_search` / `tools_bindings` vs. alternatives;
   `tools_execute` stays reserved for hosted execution) and whether to keep
   `agentstack_search` distinct (recommended: yes).
2. Transport for the runtime shim (loopback HTTP token-gated — recommended — vs.
   stdio bridge).
3. Binding language(s): TS first; Python second; both behind one generator.
4. Whether phase-1 search fully *replaces* namespaced listing or offers a
   `--legacy-list` escape hatch during migration.

## Definition of done (per the vision's bar)

Every phase ships with: CLI/tool help text, a docs recipe, dry-run where files
are touched, tests for success/refusal/idempotency, `explain` visibility, and
clear ownership rules for any generated files.

## One-line positioning

> A user with many MCP servers points their harness at `agentstack mcp` and sees
> a compact runtime surface instead of a hundred tools — discovery by `search`,
> orchestration through generated bindings the harness runs — while agentstack stays the reviewable,
> secret-safe compiler it already is.

Sources:
[Anthropic — Code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp),
[TanStack AI — Code Mode](https://tanstack.com/ai/latest/docs/code-mode/code-mode),
[Cloudflare Code Mode (via kentcdodds/kody project-intent)](https://github.com/kentcdodds/kody).
