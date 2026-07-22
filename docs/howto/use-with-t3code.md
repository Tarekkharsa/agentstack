<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Use with t3code

For anyone driving their agents through [t3code](https://github.com/pingdotgg/t3code),
the web GUI over Claude Code, Codex, Cursor, and OpenCode. Short version:
**AgentStack already governs t3code sessions — you install nothing extra and
change nothing in t3code.**

## Why it works automatically

t3code has no config surface of its own. Every session it starts loads the
underlying CLI's native configuration — Claude Code reads its user, project,
and local settings; Codex reads `CODEX_HOME`. Those are exactly the files
AgentStack renders and the hooks it installs, so the manifest, the trust gate,
and the [guard](../ENFORCEMENT.md) all apply to t3code sessions unchanged.

One thing to know: t3code's default mode for a new thread is **Full access**,
which maps to each CLI's own bypass flag (`bypassPermissions` on Claude Code,
`danger-full-access` on Codex). In that mode the CLI's built-in approval
prompts are off — the AgentStack guard hook is the **only** pre-tool-use gate
left standing. That is the point of the integration, but it means guard
coverage matters more inside t3code than anywhere else.

## Check your posture

```bash
agentstack doctor
```

When `~/.t3` exists, doctor prints a `t3code (supervisor)` section that checks
the two things able to quietly break the chain:

- **Guard coverage per provider.** If the guard hook is missing for a CLI that
  t3code drives, doctor warns that Full-access sessions on that provider run
  ungated, and names the fix (`agentstack guard install`).
- **Home overrides.** A t3code provider instance with a custom `homePath` (or
  Codex `shadowHomePath`) relocates that CLI's whole config surface —
  global-scope artifacts silently stop applying to its sessions. Doctor reads
  `~/.t3/userdata/settings.json` and flags every enabled instance that does
  this. Providers t3code supports but AgentStack has no adapter for are listed
  as unobserved.

## What a blocked call looks like

Nothing extra to set up: guard denials happen inside the session, so t3code's
own timeline shows them like any other tool outcome, and every denial lands in
the AgentStack [audit log](see-what-happened.md).

There is also an optional t3code branch (maintained in this project, pending
upstream) that renders AgentStack natively in the t3code UI: a governance
status panel in the chat header backed by `agentstack doctor --json`, and a
purpose-built "Blocked by AgentStack Guard" card in the timeline showing the
rule, the blocked target, and the audit note.

## Per-run evidence (optional)

By default t3code sessions land in the machine-global call audit. To give
each t3code session its own run identity and `events.jsonl`:

```bash
agentstack shim make claude
```

This writes an exec-through wrapper at `~/.agentstack/shims/claude`; point
the t3code provider instance's **Binary path** setting at it (agentstack
never edits t3code's settings itself). Every session that instance starts
then mints a run id before becoming the real CLI — same pid, signals, and
exit code as a direct launch. Inspect with `agentstack report runs` and
`agentstack report run <id>`; doctor's t3code section confirms shimmed
instances. One t3code session = one run.

## Limits, honestly
- t3code injects one MCP endpoint of its own (`t3-code`, for driving its
  in-app browser preview) directly into each session, outside any config file.
  It never appears in a manifest or lockfile; the guard still gates its tool
  calls at runtime.
- Doctor reads t3code's production state (`~/.t3/userdata`); a t3code built
  and run from source in dev mode keeps separate state that is not checked.
