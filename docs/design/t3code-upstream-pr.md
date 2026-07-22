# Draft PR for pingdotgg/t3code — hold until they accept contributions

Status 2026-07-22: t3code's CONTRIBUTING.md says they are not accepting
contributions yet and external PRs start `vouch:unvouched`. This is the
paste-ready PR for branch `agentstack-panel` (commit 89d1c000c) the day that
changes. Rebase onto their origin/main first — it is a fast-moving repo.

**Splitting option:** the `failureText` change in `session-logic.ts` is a
standalone general fix (hook/policy denials from ANY tool currently lose their
reason text when the call completes with `is_error` in the result instead of a
failed status). If a full vendor-integration PR is a hard sell, submit that
part alone first as a small reliability fix — their most-likely-to-accept
category — and hold the panel until there is an extension story.

---

## Title

feat(web,server): optional read-only AgentStack governance panel and guard-denial card

## Body

### What this adds

[AgentStack](https://github.com/Tarekkharsa/agentstack) is a local CLI that
trust-gates, firewalls, and audits what agent CLIs (Claude Code, Codex,
Cursor, OpenCode) may do on a machine. Because T3 Code delegates all session
config to each provider's native files, AgentStack already governs T3 Code
sessions from the outside — this PR only makes that visible in the UI:

1. **Header status panel.** An AgentStack mark in the chat-header action
   cluster opens a popover with the project's governance overview (manifest
   drift, doctor summary, gateway, secrets, policy, skills), read from
   `agentstack doctor --json`. Polls only while open (5s).
2. **Guard-denial timeline card.** A tool call blocked by AgentStack's
   pre-tool-use hook currently renders as a generic failed row. It now gets a
   purpose-built card — rule chip, blocked target, "nothing was executed" note
   — so a denial reads as protection working, not as a session error.
3. **General fix that fell out of 2:** in Full-access mode a hook-blocked call
   arrives as a *completed* tool call whose result carries `is_error: true`;
   `session-logic.ts` previously captured failure text only from
   `status: "failed"`. `failureText` is now also extracted from the result's
   own error flag (covered by a test using the observed payload verbatim).

### When AgentStack is not installed

The panel shows a one-line notice; nothing else changes. No new dependency,
no network calls, no persistent state. Binary resolution is `agentstack` on
PATH, overridable via `T3CODE_AGENTSTACK_BIN` (server env, never client
input).

### Design constraints followed

- **Clients never send paths.** `agentstack.status` takes `{projectId,
  threadId?}`; the server resolves the workspace root from its own
  projections (same pattern as `assets.createUrl`) and rejects a thread that
  does not belong to the project.
- **Read-only scope**: the RPC is gated behind `orchestration:read`.
- **CLI output is untrusted input**: args-array spawn (no shell), 15 s
  timeout, 2 MB bounded stdout, schema-decoded with graceful null on any
  mismatch; version probe cached 5 min via `Cache.make`.
- Contracts live in `packages/contracts/src/agentstack.ts` (schema-only),
  UI logic is pure and unit-tested (`agentstack-logic.ts`), all new files
  namespaced under `agentstack/` directories.

### Verification

- Focused tests: `agentstack-logic.test.ts` (doctor mapping + denial matcher
  across Claude/Codex/bypass phrasings), `AgentstackCli.test.ts` (ENOENT →
  not-installed), `session-logic.test.ts` (is_error failure text),
  `server.test.ts` green with the mocked layer.
- Typecheck clean on contracts, client-runtime, server, web; `vp lint` and
  `vp fmt --check` clean.
- Integrated pass per AGENTS.md via the `test-t3-app` flow: isolated
  `vp run dev --home-dir`, pairing URL auth, added a governed project, and
  verified the live popover (real doctor rows) and a real guard denial card
  in a session.
