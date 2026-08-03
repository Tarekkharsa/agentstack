# skills-workout — one skill set, two delivery lanes, identical bytes

Delivery is **routed**: `agentstack delivery` decides, per tool and per
capability kind, which lane a skill takes. On an MCP-capable tool skills take
the **live lane**, and that is the default — the assertion run starts by asking
`delivery` rather than assuming it. Lane B is that live lane: an agent talking
to `agentstack mcp` over stdio opens a toolset lease and pulls the skills into
its context on demand — nothing is written to disk, and the lease drops when the
connection closes. Lane A is the **rendered lane, on request**: `agentstack use
<toolset> --write` materializes the toolset's skills as symlinks in the CLI's
own native skills directory (`.claude/skills/`), for a tool that reads files or
a team that wants them committed.

This example proves the two lanes deliver **the same content**. They share one
manifest. It declares two skills inline (`api-conventions`,
`release-checklist`), and its `docs` toolset mixes one of those with a skill
that lives only in the machine's central library (`sql-review`, seeded here by
`agentstack lib add`). The `all` toolset uses the `"*"` wildcard. Running both
lanes against this single source is the whole point: whatever an agent sees
through the lease is exactly what the rendered lane would put on disk.

```bash
bash examples/projects/skills-workout/assert.sh
# or, against a specific binary:
AGENTSTACK_BIN=target/release/agentstack examples/projects/skills-workout/assert.sh
```

Requires `agentstack` on `PATH` (or `AGENTSTACK_BIN=/path/to/agentstack`, or a
built `target/release/agentstack` in this repo) and `python3`. Everything runs
in an isolated `AGENTSTACK_HOME` + `HOME` — the machine's real manifest,
library, trust store, and audit log are untouched.

## What PASS proves

**Lane A — the rendered lane (`use <toolset> --write`):**

- `use docs --write` materializes **exactly** `{api-conventions, sql-review}` as
  symlinks, and the bodies followed through those links match their sources
  byte-for-byte — the inline skill from the manifest, the other from the library.
- `use all --write` re-renders in place: `sql-review` is pruned, `release-checklist`
  is added, so `.claude/skills/` ends at exactly `{api-conventions, release-checklist}`.
- The `"*"` wildcard expands to the manifest's **inline** skills only — it does
  **not** sweep in library skills. (`sql-review` / `incident-runbook` never
  appear under `all`; `docs` only gets `sql-review` because it names it.)
- A hand-made, unmanaged `handmade-local/` skill dir dropped into
  `.claude/skills/` **survives** the re-render. Pruning only ever removes links
  AgentStack itself created; it never clobbers what it did not make.

**Lane B — the live lane, zero files (`agentstack mcp`):**

- `agentstack_lease_open({profile: "docs"})` reports `native_files_written: false`
  — the lease writes nothing to disk.
- `agentstack_list_loadable` is fenced to exactly the docs toolset's two skills
  plus the built-in `using-agentstack` operator manual — nothing else is
  visible.
- `agentstack_load` returns each skill's `SKILL.md` bytes, and tags the origin
  correctly (`api-conventions` = `manifest`, `sql-review` = `library`).
- Loading `release-checklist` — a real manifest skill, but **not** in the `docs`
  toolset — is **refused**. The fence holds, and the refused attempt leaves no
  trace in the lease's load trail.
- `agentstack_lease_status` records the trail: each load's name and the reason
  the agent gave. `agentstack_lease_close` reports `native_restore_needed: false`.

**The point:** for each skill, the bytes Lane A rendered to disk are
**identical** to the bytes Lane B loaded into the agent's context. One
manifest, two delivery lanes, zero divergence.

The script ends with a `N passed, 0 failed` summary and exits nonzero on any
mismatch, so it doubles as a CI-grade regression check. It is deterministic
across fresh sandboxes.

## What it does not claim

This example is about **skill delivery equivalence and the toolset fence**, not
runtime firewalling. The lease brokers only skill content here (the `docs`
toolset declares no servers); for the MCP tool firewall and trust gate see
`examples/malicious-repo-demo/`, and for kernel-enforced egress/filesystem
confinement see `agentstack run --sandbox`/`--lockdown`.
