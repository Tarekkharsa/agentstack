# Spike: gateway HTTP transport for sandboxed harnesses (2026-07-11)

Session 0 of the gateway-unification milestone (ROADMAP "Adopted from the
2026-07-11 strategy reviews", item 1). Three go/no-go questions, all answered
**on the maintainer's machine** (macOS, Docker Desktop 25.0.3, sandbox image =
`docker/sandbox.Dockerfile` → claude-code 2.1.207). Verdict: **GO.**

Method: a POST-only Python stub speaking minimal MCP streamable-HTTP on the
host (:8765), the sandbox image run against it with the entry rendered as
`{"type":"http","url":"http://host.docker.internal:8765/mcp","headers":{"X-Agentstack-Token":…}}`,
health-checked via `claude mcp list`. Full request log preserved in the
session transcript.

## 1. POST-only streamable HTTP — PASS

claude-code 2.1.207 completes the session against a server that never streams:

- `initialize` POST → plain `application/json` response accepted. The server
  MUST include an `Mcp-Session-Id` response header; the client echoes it (and
  `MCP-Protocol-Version: 2025-11-25`) on every subsequent request.
- `notifications/initialized` POST → answer **202** with empty body.
- The client then attempts the optional SSE channel: `GET /mcp` with
  `Accept: text/event-stream`. Answering **405** is tolerated per spec — the
  client proceeds to `tools/list` regardless. No SSE implementation needed.
- Custom headers from the rendered entry arrive on **every** request
  (including the GET) — the bearer-token check can be per-request.

So `tiny_http` in the cli crate suffices: POST dispatch + session-id header +
202 for notifications + 405 for GET. No tokio, no new dependency.

## 2. File bind-mount shadowing inside the workspace mount — PASS

`-v <ws>:/workspace:ro -v <rendered>:/workspace/.mcp.json:ro` on Docker
Desktop/macOS: the container sees only the rendered gateway entry; a stale
direct-rendered config (with a planted fake secret literal) at the same path
is fully shadowed — `grep -r` for the literal across `/workspace` finds
nothing. The overlay strategy from the plan is viable as designed.

## 3. `NO_PROXY` honoring — PASS (and required)

Control: with `HTTPS_PROXY`/`HTTP_PROXY` pointing at an unreachable proxy and
no carve-out, the gateway connection **fails** — the client does route plain
HTTP through the proxy env, confirming review objection #4 is real. With
`NO_PROXY=host.docker.internal` (both spellings set) the connection succeeds.
`execute_plan` must add the gateway host to `NO_PROXY`/`no_proxy` in the
container env.

## Extra finding (not in the plan): project-scope entries need approval

A gateway entry rendered into project-scope `.mcp.json` shows as
**"⏸ Pending approval (run `claude` to approve)"** — an interactive gate no
one can click inside the container. Registered at **user scope** (the
container's own `/root/.claude.json`) the same entry connects with no prompt.

Implication for Session 2: render the synthetic gateway entry at the
adapter's GLOBAL-scope config path, translated to the container home (e.g.
host `~/.claude.json` → container `/root/.claude.json`), delivered as one
more file bind-mount — do NOT shadow the project-scope path and rely on
approval. This also leaves the project's own `.mcp.json` untouched-but-
shadowless; if it contains direct entries with baked secrets they remain
visible in the workspace mount, so Session 2 should still shadow the
project-scope path with an empty/neutral config for the selected harness.
Approval semantics are per-harness; verify per adapter when extending beyond
claude-code.
