# The no-terminal path

The dashboard is not a viewer — it is the full product for people who don't
live in a terminal. This page walks the complete capability lifecycle —
**discover → add → secrets → enable per CLI → apply → verify → remove →
undo** — without typing a single command after launch. Every step below names
the exact pane and button, and each mutation goes through the same gates as
the CLI's `--write` (unresolved-`${REF}` blocking, validation, policy,
content scan). Secret values never reach the browser.

## 0. Launch (the one terminal moment)

```sh
agentstack dashboard              # read-write
agentstack dashboard --read-only  # browse + preview only; every write refused
```

It binds 127.0.0.1 only and prints a token-gated URL. In `--read-only` mode
every mutating endpoint answers 403 — that's enforced centrally for all POST
routes and pinned by a route-matrix test, so a future endpoint can't forget
the gate.

## 1. First run — no manifest yet

On a machine with no `agentstack.toml`, the dashboard opens a welcome screen
instead of an empty app: it lists the agent CLIs it detected and the MCP
servers already in their configs, and one button — **Initialize** — reverse-
engineers a manifest from what's already on disk, lifting inline secrets into
`${REF}`s. You start from your real setup, never a blank page.

## 2. Discover → add

**Discover** tab: search the embedded catalog and the official MCP Registry.
Adding an entry writes it to the manifest only — commit-safe `${REF}`s,
nothing executed, nothing applied yet. Custom servers go in through
**Servers → + Add MCP server** (form, not TOML).

## 3. Secrets

**Secrets** tab: every `${REF}` the manifest mentions, whether it resolves on
this machine, and from which layer (env / varlock / keychain / .env). Set a
value here and it lands in the OS keychain. Values are write-only — the API
never returns them.

## 4. Enable per CLI

**Servers** tab: the cross-harness matrix. Toggle any server per CLI, per
scope (global/project switch at the top). The **context** column shows what
each server's tool list costs in context-window tokens per session (measured
by `agentstack stats --live`, cached) — click the header to sort by cost.

## 5. Apply

**Preview & apply**: a real diff of every native config before anything is
written. Unresolved secrets block the write for that target with a banner
naming the missing ref and the fix — placeholders never reach a live config.
Writes are atomic, backed up, and recorded.

## 6. Verify

**Health** tab: the standing summary, plus **Run doctor** — the same checks
as `agentstack doctor` (manifest validation, adapters, secrets, drift,
quirks, skills, content scan, reproducibility, policy), rendered as the
familiar ✓/⚠/✗ report. **Explain trust ⓘ** on any server shows provenance,
secrets, where it writes, context cost, and safety signals.

## 7. Remove

Open a server's detail row → **Remove from stack…**. It leaves the manifest
immediately (a pack ledger tears down every member the pack installed); the
pending bar shows what the next Apply will strip from each CLI's config.

## 8. Undo

**Activity** tab: every apply, with the files it touched. **Undo** restores
the pre-write bytes (or deletes files the apply created). The manifest itself
is never touched by undo.

## 9. Analyze — read-only lenses

Two tabs surface the analysis functions that used to be CLI-only. Both are
strictly read-only: they add no mutating endpoints and stay fully available in
`--read-only` mode.

**Proxy** tab: the wire lens, the same ranked report as `agentstack proxy
report`. Once the observe-only proxy has seen traffic
(`ANTHROPIC_BASE_URL=http://127.0.0.1:8787`), it shows the per-turn tools and
token weight plus a per-capability table — tools, average tokens/turn, calls,
and a `drop / lazy` · `keep` · `watch` hint — so you can see which loaded
tools actually earn their context. With no telemetry yet it shows an explicit
empty state pointing at `agentstack proxy start`.

**Insights** tab stacks three read-only reports:

- **Optimize** — the recommendations from `agentstack optimize`, each with its
  evidence, the exact command/TOML to act on it, and why it's safe or needs a
  human. (Applying still happens in the terminal — the dashboard only shows.)
- **Analyze** — runtime call activity from the gateway audit log (ok / error /
  denied, top servers and tools) plus library dead weight: capabilities carried
  but never activated or called anywhere.
- **Stats** — the per-capability table from `agentstack stats`: activations,
  measured context cost, how many slots each is live in, and dead-weight flags.

---

Also on the dashboard: skills matrix + consolidate, typed per-CLI settings
editors, hooks, plugin recipes, profiles/sessions, and live runs (see and
kill tracked agent processes). The complete inventory is in
[reference.md](reference.md).
