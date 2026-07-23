# Draft PR for pingdotgg/t3code — hold until they accept contributions

Status 2026-07-23: t3code's CONTRIBUTING.md says they are not accepting
contributions yet and external PRs start `vouch:unvouched`. This is the
paste-ready PR for branch `agentstack-panel` (tip `c3506ad8a`, 4 commits on
top of their `main`) the day that changes. **Rebase onto their origin/main
first — it is a fast-moving repo.**

Branch commits (each self-contained; can be reordered/split):

1. `89d1c000c` read-only status popover + guard-denial card + the `is_error`
   failureText fix
2. `bd248d096` recent-calls Activity feed
3. `b228f5950` the four-tab panel (Overview/Workflow/Activity/Policy) + a
   governed-action **write** RPC
4. `c3506ad8a` richer denial card (policy chip, code block, audit-log
   deep-link)

Diffstat: 22 files, ~2370 insertions, 7 deletions. New code is namespaced
under `agentstack/` directories; touch points in shared files are small
(one import + handler in `ws.ts`, one RPC registration, one prop through
`ChatView → ChatHeader`, and the `failureText`/denial-card hook in
`session-logic.ts` / `MessagesTimeline.tsx`). **No new dependencies**
(`agentstackPanelStore` uses the `zustand` already vendored for
`composerDraftStore` et al.).

## The one decision to make before merging

Commits 1, 2, 4 are **strictly read-only** and match the existing
`packages/contracts/src/agentstack.ts` promise ("T3 Code never writes
AgentStack state"). Commit 3 adds one **write** path — a governed-action
RPC (fix drift / enable guard) — which changes that promise. It is scoped
and safe by construction (see below), but it is a product decision, not just
wiring. **Three ways to take this:**

- **Read-only only** — drop commit 3's action RPC (keep its tabs/workflow
  read RPC), preserving "never writes". Smallest ask.
- **Full panel** — take all four; the contract doc-comment is updated to
  describe the closed action set.
- **Fix alone** — the `is_error` failureText change in `session-logic.ts` is a
  standalone reliability fix (hook/policy denials from ANY tool currently lose
  their reason text when the call completes with `is_error` in the result
  rather than a failed status). It's the most-likely-to-accept slice; land it
  first and hold the rest.

---

## Title

feat(web,server): optional AgentStack governance panel, workflow monitor, and guard-denial card

## Body

### What this adds

[AgentStack](https://github.com/Tarekkharsa/agentstack) is a local CLI that
trust-gates, firewalls, and audits what agent CLIs (Claude Code, Codex,
Cursor, OpenCode) may do on a machine. Because T3 Code delegates all session
config to each provider's native files, AgentStack already governs T3 Code
sessions from the outside — this PR surfaces that governance in the UI.

**A four-tab panel** behind the AgentStack mark in the chat-header cluster
(polls only while open, 5s), all backed by real CLI output:

- **Overview** — governance rows (Manifest, Doctor, Guard, Gateway, Secrets,
  Library, Sandbox, Workflows) mapped from `agentstack doctor --json`
  sections. Rows with no honest source are omitted, never invented. A header
  **trust badge** (trusted / inert / drifted) derived from the gateway line.
- **Workflow** — declared workflows with trust/lock badges, and when one is
  running, a live monitor (stages grouped by step-label convention, per-agent
  state/role/tool-counts, pinned digest, done/running). Stage grouping is
  explicitly labelled a convention, not enforced structure; "queued" is
  omitted because it isn't derivable.
- **Activity** — the recent brokered-call feed (`report calls --json --tail`),
  argument digests only, never values.
- **Policy** — the machine-policy ceiling, verbatim.

**A guard-denial timeline card.** A tool call blocked by AgentStack's
pre-tool-use hook renders as a purpose-built card instead of a generic failed
row: a policy-dimension chip (from the `[policy.<dim>]` tag), the blocked
target in a code block, a "nothing ran — recorded in the audit log"
reassurance, and a **View in audit log** action that jumps the panel to its
Activity tab. Where a naive design would put "Allow once", there is a disabled
"Can't be overridden here" with a tooltip — a guard denial is the machine
ceiling, which the UI must not loosen.

**A general fix that fell out of the card:** in Full-access mode a
hook-blocked call arrives as a *completed* tool call whose result carries
`is_error: true`; `session-logic.ts` previously read failure text only from
`status: "failed"`. `failureText` is now also extracted from the result's own
error flag (test uses the observed payload verbatim). This helps any tool
whose failure lives in the result, not just AgentStack's.

**Governed CLI control (the one write path).** An `agentstack.action` RPC lets
the panel trigger a *closed enum* of vetted commands — `apply --write` (fix
drift) and `guard install` (enable guard) — each behind a confirmation dialog.

### When AgentStack is not installed / is older

The panel shows a one-line notice; nothing else changes. Individual surfaces
degrade independently: an older CLI without `workflow list --json` simply
yields an empty Workflow tab rather than an error. No new dependency, no
network calls, no persistent state on our side. Binary resolution is
`agentstack` on PATH, overridable via `T3CODE_AGENTSTACK_BIN` (server env,
never client input).

### Security posture

- **Clients never send paths.** Every RPC takes `{projectId, threadId?}`; a
  shared server-side resolver derives the workspace root from orchestration
  projections (same pattern as `assets.createUrl`) and rejects a thread that
  doesn't belong to the project. The CLI is never pointed at a client path.
- **Read vs write scopes.** `agentstack.status` / `.activity` / `.workflow`
  are `orchestration:read`. `agentstack.action` is `orchestration:operate`
  (same tier as `vcs.pull` / `server.signalProcess`). *Open question for you:*
  guard/apply touch the security control plane, so a dedicated
  `agentstack:admin` scope (in `AuthAdministrativeScopes`) would be stronger
  than reusing `operate` — flagged, not assumed.
- **Actions can't loosen policy, by construction.** The client sends an enum;
  the server maps it to fixed argv (never a client command line). `apply`
  re-renders configs capped by the machine ceiling and is reversible via
  `agentstack restore`; `guard install` only adds protection. Each runs behind
  a confirm dialog naming what it does.
- **No guard bypass.** Overriding a denial has no safe shape (it would produce
  an effective policy more permissive than the machine ceiling), so it is
  deliberately not offered — the card says so.
- **CLI output is untrusted input**: args-array spawn (no shell), timeouts
  (15s read / 90s action), 2 MB bounded stdout, schema-decoded with graceful
  null/empty on any mismatch; version probe cached 5 min via `Cache.make`.

### Verification

- Focused unit tests: `agentstack-logic.test.ts` (doctor→rows mapping, trust
  badge, policy rows, workflow stage/count derivations, denial matcher across
  Claude/Codex/bypass phrasings incl. the policy dimension), `AgentstackCli`
  (ENOENT → not-installed), `session-logic.test.ts` (is_error failure text),
  `server.test.ts` green with the mocked layer.
- Typecheck clean on contracts, client-runtime, server, web; `vp lint` and
  `vp fmt --check` clean.
- Integrated passes per AGENTS.md via the `test-t3-app` flow (isolated
  `vp run dev --home-dir`, pairing-URL auth): verified all four tabs against
  real doctor/activity/policy/workflow data, the fix-drift confirmation
  dialog, and — with a real agent session blocked from reading `.env` — the
  live denial card and its deep-link into the Activity tab.
