# Reference field notes

Maintainer-facing addenda split out of [the feature reference](../reference.md):
operational edge cases, crate-level caveats, and implementation rationale that
back the reference but are too deep for it. Nothing here is required to *use*
agentstack — reach for it when a corner case bites or you are reading the code.
(This page lives under `docs/design/` and is not compiled into a site page; the
docs-site build rewrites links to it as GitHub URLs.)

## Launch timing and switching

AgentStack assumes harness-native configuration is established **when the CLI
launches**. `use` and `session start` write a profile's native MCP config and
skills to disk, but a CLI that is already running may not observe the change
until it is relaunched. To switch profiles deterministically for a running
CLI — `use profile-B --write` rewrites the files, then relaunch it — rather
than assuming a live reload. Live, in-process switching is the MCP lease path's
job, not the native-file path's.

## Session and run recovery

`session end`/`end --all` and `run`'s auto-revert restore the pre-session state.
Two edge cases:

- **Force-killed parent.** If the parent `agentstack run` process is killed
  (or the machine stops) before it can revert, cleanup cannot execute; recover
  with `session end` or `session end --all`.
- **Skill-restore exactness.** Server files are snapshotted for exact restore.
  For skills, the current implementation records the names a session *newly
  added*; replacing an already-managed skill with the same name is an edge case
  that is not yet snapshotted, so restore of that specific case is not promised
  to be byte-exact.

## Lease survival across a mid-connection change

If manifest or lock bytes change while an `--auto-project` connection is open,
AgentStack empties the **live** gateway — no further server spawns, secret
resolution, or bundle content. The **in-memory lease object itself** can still
be inspected, and `lease_freeze` can still propose a manifest profile from it,
precisely because a lease serves no bundle content, resolves no secret, spawns
no server, and touches no file. Any renewed activity still requires fresh review,
locking where needed, and trust.

## Zero-files gateway: always-on manual

agentstack's own manual — the bundled `using-agentstack` skill — is always
loadable through the control plane: it appears in `agentstack_list_loadable` even
with no project manifest, in untrusted sessions, and through session fences,
served from the copy embedded in the binary (a project's own `using-agentstack`
skill overrides it). The `initialize` handshake also carries an ambient skill
index in the server's `instructions` field — every loadable skill (name +
one-line description) — subject to the same trust gate (untrusted projects list
names only) and any active session fence.

## Central library: server definitions and bundled catalog

- `lib add-server <name> --file <definition.toml>` stores a standalone server
  definition; `lib add-server <name> --from-manifest` lifts an existing inline
  `[servers.*]` entry into the library. Both keep `${REF}`s intact and **warn on
  literal secret-looking values at add time** (surfaced, not scrubbed or
  blocked) — an earlier checkpoint than `lib sync`'s fail-closed push-time gate.
- The bundled catalog (`crates/cli/catalog/skills/`) ships ready-made skills
  including `run-codex`, `sync-library`, `analyze-usage`, `route-by-cost`, and
  `using-agentstack`, among others; `search` finds them across providers.
- Every central-library flow is exercised by
  `examples/sandbox/demo-central-library.sh`, a sandboxed demo that never
  touches your real provider folders.

## `add skill`: discovery and staging

`agentstack add skill <source>` discovery scans the ecosystem's conventional
locations (repo root, `skills/` and its dot-variants, the agent-convention dirs)
one level deep, two for `skills/<category>/<skill>` catalogs. When nothing
conventional exists, a depth-5 fallback walk runs — its hits are announced with
their paths and are never auto-selected. Duplicate skill names across locations
are an error naming every path. The dry run stages the fetch under
`~/.agentstack/stage/…` (removed on exit) and never touches the manifest, lock,
or content store; `--write` promotes the staged clone rename-only, so the scanned
bytes land verbatim.

## Orphaned digest cache

Skill and library content digests always hash current bytes; there is no digest
cache on the verification path. Older versions kept a stat-fingerprint cache and
may leave a harmless orphaned `~/.agentstack/digest-cache.json`; it is unused and
safe to delete.

## Wire proxy internals

The `agentstack proxy` wire relay is the built-in version of the hand-rolled
logging proxy from [*How to kill the bloat in Claude Code's system
prompt*](https://www.aihero.dev/how-to-kill-the-bloat-in-claude-codes-system-prompt),
and the on-wire complement to `src/footprint.rs`'s static counter.

- **What it does per request.** For each `/v1/messages` request it walks the
  `tools` array and buckets every tool into its capability —
  `mcp__<server>__<tool>` → `<server>`, everything else (`Read`, `Bash`,
  `Task`, …) → `builtin` — summing each bucket's estimated per-turn token cost
  (same `estimate_tokens` heuristic as the static footprint). Off the response
  it captures best-effort usage numbers and the tool NAMES the model actually
  called, for both non-streamed JSON and streamed (SSE) responses. The SSE path
  tees the stream through a pass-through reader: bytes reach the client
  unchanged and undelayed while a side buffer absorbs `tool_use` names and
  usage — so streamed turns report real `calls` (previously always 0 under
  streaming).
- **Guardrails.** The proxy is **observe-only** (it never injects or
  mutates the tools/system block, so the prompt-prefix cache stays warm), all
  accounting is **best-effort and fail-open** (a parse hiccup never delays or
  fails the proxied request — a forwarding error returns a 502 but keeps the
  accept loop alive), and auth headers pass through untouched. Each request is
  handled on its own thread so a long-lived SSE stream can't block concurrent
  calls from parallel subagents or background token-count/compaction requests.

## `tools_execute` cancellation

Cancelling an execution kills the **entire process tree**: the executor's
container and its children are torn down (bounded by the 32-PID limit), so a
cancelled or timed-out run leaves no orphaned guest processes.

## `tools_execute` review status

A focused implementation review and regression-hardening pass completed on
2026-07-13. The surface remains experimental while image provenance/SBOM
publication and longer-running soak evidence remain outstanding. The full
security accounting is the
[enforcement matrix](../ENFORCEMENT.md#experimental-tools_execute); the design
rationale is the
[`tools_execute` threat model](tools-execute-threat-model.md) and the
[runtime/ownership ADR](adr-tools-execute-runtime.md).
