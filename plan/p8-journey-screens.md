# P8 — the six journey screens, captured

Every terminal block on this page is machine-captured stdout+stderr from a
binary built for this page. Nothing here is retyped, abridged, or
reconstructed. Where a screen could NOT be captured, it says so in place and
says why; it is not filled in from the source.

## Provenance

| | |
|---|---|
| Commit | `9aef01e` (`fix/followups-2026-08-06`), exported with `git archive HEAD` so no other agent's uncommitted edits are in it |
| **Binary A** | `agentstack 0.18.0-rc.2 (sandbox: no)` — `cargo build -p agentstack --release`, default features |
| **Binary B** | `agentstack 0.18.0-rc.2 (sandbox: yes)` — `cargo build -p agentstack --features sandbox --release`, same commit |
| Machine | macOS (darwin 25.5.0), Docker present (`docker info` exit 0) |
| Environment | isolated `HOME` and `AGENTSTACK_HOME` under a scratch dir; a ten-line fake `claude` first on `PATH` |
| Date | 2026-08-06 |

Both binaries were built **before** any capture on this page. Every block
carries the binary that produced it, because the two disagree about
`--sandbox`.

**Exit codes** were read from `$?` on the line after the command, never through
a pipe. Each block ends with the `exit=` line the harness appended.

**Two disclosed transformations, applied by the capture harness to every block
alike:**

1. **ANSI colour escapes stripped.** The binary emits them unconditionally —
   see gap **P8-G1** — so raw capture files are unreadable as text.
2. **The scratch path rewritten to `<lab>`.** A literal substring replacement
   of the lab root; nothing else in any line was touched.

Nothing else was edited. Line breaks, spacing, glyphs, digests and run ids are
the binary's own.

## What the record said, and what is true

`TODO.md` item 11 says P8 stays open because "`run` and `workflow` were never
exercised, and six journey screens are sketches". Both halves were re-tested
today against the binary above.

- **`run` and `workflow` both run.** All five `run` postures and a full
  `workflow` lifecycle were exercised, including a real Docker sandbox and a
  real lockdown run. Nothing was blocked by missing infrastructure.
- **The screens were sketches in a narrower and more specific sense than the
  record implies.** `plan/p8-scope.md`'s appendix, which is headed *"Appendix —
  raw evidence"*, is **not raw**: it abbreviates digests to `sha256:11a396cf…`
  where the binary prints all 64 hex characters, elides whole lines as `…`,
  and in one place substitutes a description for the output
  (`[the full BOUNDED / NOT BOUNDED block, printed verbatim]`). Its findings
  hold up — every one I re-tested reproduced — but its transcripts are
  reconstructions presented under a raw-evidence heading. That is the failure
  mode this project already named: a provenance claim is worse than a stale
  sample. **This page supersedes that appendix as the capture of record.**

---

# Screen 1 — `use` / `use --write`

The most-run screen of the six: activate a toolset into every detected harness.

## 1a. Dry run, on a project that has not been trusted

The refusals are the interesting half. The trust gate (G1) holds — no skill
file is written — and each blocked harness names the blocker.

*Binary A · cwd `app` · capture `12-use-untrusted`*

```text
$ agentstack use backend
Activating toolset 'backend' (scope: project) — 1 server, 1 skill

Claude Code
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  ✗ refusing to materialize skills: project at <lab>/app is not trusted — review and `agentstack trust .` before putting its words into an agent's context ('greet')

Codex CLI
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  ✗ refusing to materialize skills: project at <lab>/app is not trusted — review and `agentstack trust .` before putting its words into an agent's context ('greet')

GitHub Copilot CLI
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  · (no skills dir at this scope for this CLI — 1 skill not materialized)

OpenCode
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  · (skills not supported by this CLI — 1 skill not materialized)

Pi
  servers: no project scope
  ✗ refusing to materialize skills: project at <lab>/app is not trusted — review and `agentstack trust .` before putting its words into an agent's context ('greet')

ℹ MCP servers for Claude Code, Codex CLI, GitHub Copilot CLI, OpenCode are routed to the live lane — `use` does not write them.
  · nothing is being served yet — Claude Code, Codex CLI, GitHub Copilot CLI, OpenCode have no bridge registered.
  → register the bridge: agentstack x gateway connect --all --write
  → or write files anyway: agentstack x delivery render-locally --write

Dry run. Re-run with --write to apply.
exit=0
```

## 1b. `--write` on the same untrusted project

Writes nothing, exits 1, names the blocker per target.

*Binary A · cwd `app-untrusted` · capture `14-use-write-untrusted`*

```text
$ agentstack use backend --write
Activating toolset 'backend' (scope: project) — 1 server, 1 skill
  ↩ undo: `agentstack x restore --last --write`

Claude Code
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  ✗ refusing to materialize skills: project at <lab>/app-untrusted is not trusted — review and `agentstack trust .` before putting its words into an agent's context ('greet')
  ✗ skills not materialized — the project has not been trusted for this content

Codex CLI
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  ✗ refusing to materialize skills: project at <lab>/app-untrusted is not trusted — review and `agentstack trust .` before putting its words into an agent's context ('greet')
  ✗ skills not materialized — the project has not been trusted for this content

GitHub Copilot CLI
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  · (no skills dir at this scope for this CLI — 1 skill not materialized)

OpenCode
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  · (skills not supported by this CLI — 1 skill not materialized)

Pi
  servers: no project scope
  ✗ refusing to materialize skills: project at <lab>/app-untrusted is not trusted — review and `agentstack trust .` before putting its words into an agent's context ('greet')
  ✗ skills not materialized — the project has not been trusted for this content

ℹ MCP servers for Claude Code, Codex CLI, GitHub Copilot CLI, OpenCode are routed to the live lane — `use` does not write them.
  · nothing is being served yet — Claude Code, Codex CLI, GitHub Copilot CLI, OpenCode have no bridge registered.
  → register the bridge: agentstack x gateway connect --all --write
  → or write files anyway: agentstack x delivery render-locally --write

⚠ activated 'backend' on 4 targets (wrote 0); 3 targets BLOCKED: Claude Code, Codex CLI, Pi
error: 3 targets blocked — each ✗ above names the blocker
exit=1
```

## 1c. The happy path — trusted, locked, `--write`

This is the screen the record had never observed. Five harnesses, three of
which take skills at this scope; the servers go to the live lane and `use`
deliberately writes none of them.

*Binary A · cwd `app` · capture `31-use-write`*

```text
$ agentstack use backend --write
Activating toolset 'backend' (scope: project) — 1 server, 1 skill
  ↩ undo: `agentstack x restore --last --write`

Claude Code
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  ✓ 1 skill → <lab>/app/.claude/skills

Codex CLI
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  ✓ 1 skill → <lab>/app/.agents/skills

GitHub Copilot CLI
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  · (no skills dir at this scope for this CLI — 1 skill not materialized)

OpenCode
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  · (skills not supported by this CLI — 1 skill not materialized)

Pi
  servers: no project scope
  ✓ 1 skill → <lab>/app/.pi/skills

ℹ MCP servers for Claude Code, Codex CLI, GitHub Copilot CLI, OpenCode are routed to the live lane — `use` does not write them.
  · nothing is being served yet — Claude Code, Codex CLI, GitHub Copilot CLI, OpenCode have no bridge registered.
  → register the bridge: agentstack x gateway connect --all --write
  → or write files anyway: agentstack x delivery render-locally --write

✓ activated 'backend' — wrote skills to 3 locations; no server configs changed.
exit=0
```

---

# Screen 2 — `x up`

`up` is the new-machine command: detect harnesses, verify against the lock,
render. It owns no writing path of its own — it drives `apply`.

## 2a. Trusted project, **no gateway bridge registered**

Every capability here routes to the live lane, and with no bridge registered
nothing can be delivered. `up` adopts `apply --write`'s exit, so this is
**exit 1**.

*Binary A · cwd `app` · capture `32-up`*

```text
$ agentstack x up
found harnesses     Claude Code · Codex CLI · GitHub Copilot CLI · OpenCode · Pi
  ✓ greet cached (path)

✓ lockfile up to date.
your environment    1 toolset · 1 skill · 1 server · 1 skill source verified against lock
rendered
Scope: project

Claude Code
  · MCP servers are planned live (not connected), not written — nothing for `apply` to render here

Codex CLI
  · MCP servers are planned live (not connected), not written — nothing for `apply` to render here

GitHub Copilot CLI
  · MCP servers are planned live (not connected), not written — nothing for `apply` to render here

OpenCode
  · MCP servers are planned live (not connected), not written — nothing for `apply` to render here

Pi — no project MCP server config, skipping servers here

Nothing for the rendered lane here — see above.

next: agentstack x gateway connect --all --write
error: rendering stopped early — nothing was delivered: every capability here is routed to the live lane and no bridge is registered — register the bridge: agentstack x gateway connect --all --write · or write files anyway: agentstack x delivery render-locally --write
exit=1
```

## 2b. The same project, **after** `agentstack x gateway connect --all --write`

One command changes the run from a failure to a verified setup. This is the
screen a user on a working machine sees.

*Binary A · cwd `app` · capture `41-up-after-bridge`*

```text
$ agentstack x up
found harnesses     Claude Code · Codex CLI · GitHub Copilot CLI · OpenCode · Pi
  ✓ greet cached (path)

✓ lockfile up to date.
your environment    1 toolset · 1 skill · 1 server · 1 skill source verified against lock
rendered
Scope: project

Claude Code
  · MCP servers are served live, not written — nothing for `apply` to render here

Codex CLI
  · MCP servers are served live, not written — nothing for `apply` to render here

GitHub Copilot CLI
  · MCP servers are served live, not written — nothing for `apply` to render here

OpenCode
  · MCP servers are served live, not written — nothing for `apply` to render here

Pi — no project MCP server config, skipping servers here

Nothing for the rendered lane here — see above.

next: nothing to repair — this setup is verified
exit=0
```

---

# Screen 3 — `run`

Five levels were exercised. The first four need neither Docker nor the
`sandbox` feature; the last two need both.

## 3a. `--plan` — the whole protected plan, mutating nothing

*Binary A · cwd `app` · capture `50-run-plan`*

```text
$ agentstack run claude-code --toolset backend --plan
→ plan for `run claude-code --locked` (nothing will be mutated)
  posture: HOST / PROTECTED
  ℹ protected host run: content trust, strict lock verification, and policy admission are enforced BEFORE launch, and decisions are recorded. Not kernel isolation: the harness runs as you, on the host; the harness/interpreter binary itself is an unpinned $PATH executable; evidence is a cooperative local audit trail. Use --sandbox/--lockdown for runtime containment.
  ✓ no ambient user/global-scope MCP entries for this harness — nothing is reachable around the gateway at that scope. Host-guard hooks apply where the machine [guard] config installed them (cooperative).
  ✓ toolset fence: 'backend' — evaluation runs against this toolset's server subset
  ✓ trust: explicitly trusted
  ✓ locked inputs: 1 skill, 0 instructions, 1 server, 0 executable pins, 0 extensions verified
  ✓ rendered extensions: 0 verified, 0 not rendered
  ✓ policy: declared requests fit under the machine ceiling
  ℹ commitment key: will be created on first live run
  proposed grant:
    project: <lab>/app
    harness: claude-code (0 redacted arguments)
    servers: filesystem
    inputs: 1 skill, 0 instructions, 0 executable pins, 0 extensions
    rendered extensions: 0 verified, 0 not rendered
    digest: (bound on first live run, once the commitment key exists)
✓ live launch would proceed
exit=0
```

## 3b. A real governed headless run

The fake `claude` on `PATH` answers the prompt; its stdout is relayed and
recorded by digest.

*Binary A · cwd `app` · capture `51-run-prompt`*

```text
$ agentstack run claude-code --toolset backend --prompt In 6 words say the rule
▶ launching claude-code with --locked…
  ✓ headless: prompt delivered as one argv element (no shell); it is committed verbatim into the frozen grant's invocation; stdout is captured (cap 1024 KiB), relayed here, and recorded by digest only
  posture: HOST / PROTECTED
  ℹ protected host run: content trust, strict lock verification, and policy admission are enforced BEFORE launch, and decisions are recorded. Not kernel isolation: the harness runs as you, on the host; the harness/interpreter binary itself is an unpinned $PATH executable; evidence is a cooperative local audit trail. Use --sandbox/--lockdown for runtime containment.
  ✓ no ambient user/global-scope MCP entries for this harness — nothing is reachable around the gateway at that scope. Host-guard hooks apply where the machine [guard] config installed them (cooperative).
  ✓ toolset fence: 'backend' — the gates, grant, and bridge see only this toolset's servers; no native session state is applied under --locked
  ✓ trust: explicitly trusted
  ✓ locked inputs: 1 skill, 0 instructions, 1 server, 0 executable pins, 0 extensions verified
  ✓ rendered extensions: 0 verified, 0 not rendered
  ✓ policy: declared requests fit under the machine ceiling
  ✓ authority grant frozen: sha256:d87d39bf3a9c284516fb072f8050a4b2b6f12e1c6e68959ed4af002ac58a69c7
  ✓ run grant handed to the gateway (<lab>/home/.agentstack/runs/r-c6f9143d35/grant.json)
  ✓ per-run MCP config injected via harness flags (<lab>/home/.agentstack/runs/r-c6f9143d35/mcp-config.json); the shared project config is untouched
Nothing runs until it is trusted

See what happened: `agentstack x report run r-c6f9143d35`
exit=0
```

Its evidence, replayed:

*Binary A · cwd `app` · capture `81-report-run`*

```text
$ agentstack x report run r-c6f9143d35
Run r-c6f9143d35
  Locked run  claude-code · HOST / PROTECTED
    ✓ trust  (sha256:8b81955ef5ee110b341e1aac0d2fd22c766ce3c19599e29c1e42b625bc2f9bd5)
    ✓ locked-verify
    ✓ rendered-verify
    ✓ policy-admission
    ✓ grant frozen: sha256:d87d39bf3a9c284516fb072f8050a4b2b6f12e1c6e68959ed4af002ac58a69c7
    ✓ headless output: 33 bytes · sha256:831289e43c519cdf184d0f197c2fbd546e6a7aa2abe9d4cd24858a6e2a018d7d
    ✓ completed · exit 0 · 472ms
exit=0
```

## 3c. `--sandbox` on a build without the feature — the refusal

This is the obstacle the original record hit. It is a build-configuration
fact, not a product limit: published release binaries ship with the feature.

*Binary A · cwd `app` · capture `52-run-sandbox`*

```text
$ agentstack run claude-code --toolset backend --sandbox
error: this build has no sandbox support — nothing was launched

  it was compiled without the optional `sandbox` feature, so --sandbox and --lockdown have no container backend to start
  rebuild it with:  cargo build --features sandbox
  or install a published release binary — those ship with it
  either way, a sandbox run also needs a running Docker daemon
exit=1
```

`--lockdown` gives the same refusal at the same exit code (capture
`53-run-lockdown`).

## 3d. The Docker-less sandbox plan — on the **same** feature-less binary

`--plan` assembles and prints the whole sandbox decision — posture, the mount
mode **and the policy reason for it**, the egress route, the command — with
neither the feature nor a daemon. A plan is a claim about what would happen,
not an enforcement.

*Binary A · cwd `app` · capture `54-run-sandbox-plan`*

```text
$ agentstack run claude-code --toolset backend --sandbox --plan
▶ sandboxing claude-code (run r-08564bcc76) — bundle trusted
  posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
  workspace: <lab>/app → /workspace read-only — no [policy.filesystem] write scope covers the workspace (sandbox workspace writes are deny-by-default)
  🛡 egress is routed through the AgentStack proxy; review it after with `agentstack x report run r-08564bcc76`.
  command: claude
exit=0
```

And the lockdown plan, which `docs/howto/lock-down-a-run.md` tells the reader
to run first. Its "needs no Docker" claim is **confirmed**:

*Binary A · cwd `app` · capture `57-run-sandbox-lockdown-plan`*

```text
$ agentstack run claude-code --sandbox --lockdown --plan
▶ sandboxing claude-code (run r-7765d02a83) — bundle trusted
  posture: LOCKDOWN / ENFORCED · NO DIRECT ROUTE
  workspace: <lab>/app → /workspace read-only — no [policy.filesystem] write scope covers the workspace (sandbox workspace writes are deny-by-default)
  🔒 lockdown: no host route, no internet — the container's only peer is the egress sidecar. Review it with `agentstack x report run r-7765d02a83`.
  command: claude
exit=0
```

## 3e. A real sandbox run — Binary B, real Docker

First against a stock `alpine:3`, which proves the container, gateway and
proxy all come up and fails only because alpine carries no `claude`:

*Binary B · cwd `app` · capture `91-sbx-run-alpine`*

```text
$ agentstack run claude-code --toolset backend --sandbox
▶ sandboxing claude-code (run r-cfbda97b44) — bundle trusted
  posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
  workspace: <lab>/app → /workspace read-only — no [policy.filesystem] write scope covers the workspace (sandbox workspace writes are deny-by-default)
  🛡 egress is routed through the AgentStack proxy; review it after with `agentstack x report run r-cfbda97b44`.
gateway: proxying 1 frozen server from the run plan
  ✓ MCP tool calls routed through the gateway (tool policy enforced, calls recorded)
error: running the sandbox container (image `alpine:3`). If that image is missing, build a runner from docker/sandbox.Dockerfile and set AGENTSTACK_SANDBOX_IMAGE to its tag.: sandbox backend: Docker responded with status code 400: failed to create task for container: failed to create shim task: OCI runtime create failed: runc create failed: unable to start container process: exec: "claude": executable file not found in $PATH: unknown
exit=1
```

Then against a three-line runner image carrying a fake `claude` that tries to
write to the mounted workspace. **The kernel refuses the write** — this is the
containment claim, observed rather than asserted:

*Binary B · cwd `app` · capture `92-sbx-run-fake`*

```text
$ agentstack run claude-code --toolset backend --sandbox
▶ sandboxing claude-code (run r-4d0ff68249) — bundle trusted
  posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
  workspace: <lab>/app → /workspace read-only — no [policy.filesystem] write scope covers the workspace (sandbox workspace writes are deny-by-default)
  🛡 egress is routed through the AgentStack proxy; review it after with `agentstack x report run r-4d0ff68249`.
gateway: proxying 1 frozen server from the run plan
  ✓ MCP tool calls routed through the gateway (tool policy enforced, calls recorded)
fake harness running inside the sandbox container
write test:
touch: /workspace/pwned: Read-only file system
workspace write REFUSED by the kernel (read-only bind)

✓ sandbox exited cleanly.
See what happened: `agentstack x report run r-4d0ff68249`
exit=0
```

## 3f. A real lockdown run — Binary B, real Docker

*Binary B · cwd `app` · capture `93-sbx-lockdown-fake`*

```text
$ agentstack run claude-code --toolset backend --lockdown
▶ sandboxing claude-code (run r-5cdf43cea5) — bundle trusted
  posture: LOCKDOWN / ENFORCED · NO DIRECT ROUTE
  workspace: <lab>/app → /workspace read-only — no [policy.filesystem] write scope covers the workspace (sandbox workspace writes are deny-by-default)
  🔒 lockdown: no host route, no internet — the container's only peer is the egress sidecar. Review it with `agentstack x report run r-5cdf43cea5`.
gateway: proxying 1 frozen server from the run plan
  ✓ MCP tool calls routed through the gateway (tool policy enforced, calls recorded)
fake harness running inside the sandbox container
write test:
touch: /workspace/pwned: Read-only file system
workspace write REFUSED by the kernel (read-only bind)

✓ sandbox exited cleanly.
See what happened: `agentstack x report run r-5cdf43cea5`
exit=0
```

Its evidence:

*Binary B · cwd `app` · capture `94-report-lockdown`*

```text
$ agentstack x report run r-5cdf43cea5
Run r-5cdf43cea5
  Posture   LOCKDOWN / ENFORCED · NO DIRECT ROUTE
  Sandbox   agentstack-p8/runner:fake   workspace <lab>/app
  Wall time 0s sandbox
  Exit      0
exit=0
```

## 3g. The gate, when trust has drifted

`docs/howto/lock-down-a-run.md` quotes this refusal. Reproduced by trusting the
project and then appending one comment line to its manifest. The doc's sample
matches the binary word for word:

*Binary A · cwd `app-drift` · capture `59b-run-plan-truly-drifted`*

```text
$ agentstack run claude-code --toolset backend --plan
→ plan for `run claude-code --locked` (nothing will be mutated)
  posture: HOST / PROTECTED
  ℹ protected host run: content trust, strict lock verification, and policy admission are enforced BEFORE launch, and decisions are recorded. Not kernel isolation: the harness runs as you, on the host; the harness/interpreter binary itself is an unpinned $PATH executable; evidence is a cooperative local audit trail. Use --sandbox/--lockdown for runtime containment.
  ✓ no ambient user/global-scope MCP entries for this harness — nothing is reachable around the gateway at that scope. Host-guard hooks apply where the machine [guard] config installed them (cooperative).
  ✓ toolset fence: 'backend' — evaluation runs against this toolset's server subset
  ✗ trust: configuration changed since it was trusted
  ✓ locked inputs: 1 skill, 0 instructions, 1 server, 0 executable pins, 0 extensions verified
  ✓ rendered extensions: 0 verified, 0 not rendered
  ✓ policy: declared requests fit under the machine ceiling
  proposed grant:
    project: <lab>/app-drift
    harness: claude-code (0 redacted arguments)
    servers: filesystem
    inputs: 1 skill, 0 instructions, 0 executable pins, 0 extensions
    rendered extensions: 0 verified, 0 not rendered
error: a live `run claude-code --locked` would be REFUSED — 1 blocker:
  [trust] configuration changed since it was trusted — re-review and re-trust. If you changed pinned inputs, run `agentstack lock --write` first — new pins re-gate trust.
exit=1
```

---

# Screen 4 — `x workflow explain`

## 4a. Untrusted — the gate

*Binary A · cwd `wf` · capture `10-wf-explain-untrusted`*

```text
$ agentstack x workflow explain mapreduce-acceptance
error: refusing to normalize workflows: <lab>/wf is not trusted — nothing from an untrusted bundle normalizes or is invocable; review and grant with `agentstack trust .`
exit=1
```

## 4b. Trusted

*Binary A · cwd `wf` · capture `62-wf-explain`*

```text
$ agentstack x workflow explain mapreduce-acceptance
workflow  mapreduce-acceptance
pinned    d9ffcc15f3bba4625688137da0afa280eb889873e657dc3decb324e0c2f5f5b6

ceilings  max_agents=6  max_wall_seconds=300  concurrency=4

roles
  mapper               concurrent
                       harness=claude-code  model=(harness default)  effort=(harness default)
  reducer              concurrent
                       harness=claude-code  model=(harness default)  effort=(harness default)
  verifier             concurrent
                       harness=claude-code  model=(harness default)  effort=(harness default)

3 agent() call sites in the pinned source.
Sites, not calls: one site inside a loop or .map() runs once per item, so the actual
fan-out is data-dependent. The enforced bound on TOTAL spawns is max_agents=6 — the
engine refuses the call past it, per call.
exit=0
```

---

# Screen 5 — `x workflow report`

The one screen of the six with no automated assertion anywhere in the
workspace. It was run end to end here. First the workflow itself:

*Binary A · cwd `wf` · capture `70-wf-run`*

```text
$ agentstack x workflow run mapreduce-acceptance
▶ workflow 'mapreduce-acceptance' admitted: run w-f92c3a7a36, 3 roles, effective ceilings max_agents=6 max_wall_seconds=300, concurrency cap 4
◆ Map
  ▶ agent #1 [map:2] role=mapper harness=claude-code
  ▶ agent #2 [map:3] role=mapper harness=claude-code
  ▶ agent #0 [map:1] role=mapper harness=claude-code
  ✓ agent #2 completed (run r-24a739174a, grant sha256:213255a3b10536a4054387f0e68d8e1a70ff0482b953187e60b9d14eb116b936)
  ✓ agent #0 completed (run r-f60957b47b, grant sha256:61ada4c054587232cf8d6e24ee8faa1cd4d4235432042765e3952bab4712e6f8)
  ✓ agent #1 completed (run r-7f8b4dfda1, grant sha256:2897daaa4029896b9ed5d0da81b57f4b5e5469c112b32d0e3c432cf29e97ca96)
  · map outputs: Nothing runs until it is trusted | Nothing runs until it is trusted | Nothing runs until it is trusted
◆ Reduce
  ▶ agent #3 [reduce] role=reducer harness=claude-code
  ✓ agent #3 completed (run r-5cc79a0b5b, grant sha256:943b201468d16b7b8323846e582446484b74a6fa28cfcec9c38a4b66fd10af51)
  · reduced: AgentStack fails closed: policy only narrows, untrusted content stays inert, secrets never serialize.
◆ Verify
  ▶ agent #4 [verify] role=verifier harness=claude-code
  ✓ agent #4 completed (run r-c4a6595412, grant sha256:30c6c631a4894fa5824fcea562d48ab0b080979a617de808512c62517f55cdce)
{
  "pass": true,
  "mapOutputs": [
    "Nothing runs until it is trusted",
    "Nothing runs until it is trusted",
    "Nothing runs until it is trusted"
  ],
  "reduced": "AgentStack fails closed: policy only narrows, untrusted content stays inert, secrets never serialize.",
  "verdict": "CONFIRMED captures all three rules"
}
exit=0
```

Five distinct grant digests, three concurrent map children, 0.2 s wall against
the fake harness.

*Binary A · cwd `wf` · capture `71-wf-runs`*

```text
$ agentstack x workflow runs
RUN            WORKFLOW                 OUTCOME      AGO      DURATION   STEPS  RESUMABLE
w-f92c3a7a36   mapreduce-acceptance     completed    0s       0.2s       5      false
exit=0
```

Then the report. Note the taint mark on step #4 and the verbatim posture
block — the two things worth an assertion:

*Binary A · cwd `wf` · capture `72-wf-report`*

```text
$ agentstack x workflow report w-f92c3a7a36
Workflow run w-f92c3a7a36: 'mapreduce-acceptance'
  script digest   d9ffcc15f3bba4625688137da0afa280eb889873e657dc3decb324e0c2f5f5b6
  grant digest    da858a942191ddb34bc09ee61ea2c9d7857a54b90d22b672e5012cdc24734e48
  args digest     29d57ca0ce5c11dac2a7e4b922caf006a868096064ea721c6075dd208aadd6d0
  effective ceilings  max_agents=6 max_wall_seconds=300

Steps:
  #0 role=mapper [map:1] child=r-f60957b47b
     child: grant=sha256:61ada4c054587232cf8d6e24ee8faa1cd4d4235432042765e3952bab4712e6f8 posture=host-protected outcome=completed (exit 0) usage=unavailable
     step:  completed
  #1 role=mapper [map:2] child=r-7f8b4dfda1
     child: grant=sha256:2897daaa4029896b9ed5d0da81b57f4b5e5469c112b32d0e3c432cf29e97ca96 posture=host-protected outcome=completed (exit 0) usage=unavailable
     step:  completed
  #2 role=mapper [map:3] child=r-24a739174a
     child: grant=sha256:213255a3b10536a4054387f0e68d8e1a70ff0482b953187e60b9d14eb116b936 posture=host-protected outcome=completed (exit 0) usage=unavailable
     step:  completed
  #3 role=reducer [reduce] child=r-5cc79a0b5b
     child: grant=sha256:943b201468d16b7b8323846e582446484b74a6fa28cfcec9c38a4b66fd10af51 posture=host-protected outcome=completed (exit 0) usage=unavailable
     step:  completed
  #4 role=verifier [verify] child=r-c4a6595412 — taint: prompt embeds output of #3
     child: grant=sha256:30c6c631a4894fa5824fcea562d48ab0b080979a617de808512c62517f55cdce posture=host-protected outcome=completed (exit 0) usage=unavailable
     step:  completed

Outcome: done (151 ms)

Honest posture (§12.2, verbatim):
Precisely: this is a **compile-time reach** boundary (Boa's code cannot *call* those APIs), not a **runtime memory** boundary. The `workflow` crate links into the `agentstack` process, whose address space also holds the `CommitmentKey` and secrets resolved in-flight by the gateway, so a Boa memory-safety bug is a whole-process concern, not a contained one — the compile edge stops authority reach, only the WASM fallback (§12.2) would add runtime isolation. This is the honest reading of "confined."

One residual the "human-reviewed script" framing must not hide, because it is the surface v1 actually keeps: Boa's **parser** only ever sees the trusted pinned script, but Boa's **runtime** processes untrusted string *data* — `agent()` results are model output and `args` come from the invoker, and a trusted script may run string/regex builtins over them (`regress`, the backtracking regex engine, on attacker-influenced input). That is far narrower than `tools_execute` (which evaluates hostile *code*), and disabling dynamic compilation (`ensure_can_compile_strings`) means hostile data can never *become* code — but a runtime/regex bug on hostile string data is reachable, and it is exactly the class the WASM fallback would contain. State it in the posture label; it does not block v1.

The hard backstop must therefore be **out-of-thread**: a watchdog thread (or `SIGALRM`) that force-exits the process on wall-clock overrun regardless of what the drive thread is doing; "the CLI records `WorkflowFailed` and exits" is only true if a thread that is *not* stuck in Boa does the recording and the exit. So even a stalled builtin slice cannot outlive the run — via the watchdog, not the cooperative check, and the watchdog arms a no-I/O exit path before its own best-effort reporting, so a blocked write, a hung filesystem, or a contended lock can delay the honest reporting but cannot keep the process alive.

What is bounded, stated precisely because a partial bound that reads as total is exactly the failure "claims match enforcement" exists to prevent. BOUNDED: every path by which untrusted input reaches the interpreter heap — the invoker's `args` and every child result cross a depth-bounded JSON boundary under the resident-result ceiling, and `phase()`/`log()` output is capped per line and per run; the one allocation a script can name directly (`ArrayBuffer`/`SharedArrayBuffer`) is capped by an engine-owned ceiling; and invocations of the natives agentstack installs are charged against a run-total ceiling. NOT BOUNDED: there is still no JS heap cap, so a trusted, reviewed, pinned script that allocates on purpose (doubling a string in a loop) is bounded by nothing here; and work inside a single Boa built-in — a backtracking regex, a large `sort`, `String.prototype.repeat` — ticks no counter at any setting, because Boa 0.21 exposes no instruction counter or interrupt hook to build one on. Both residuals are defects in REVIEWED content rather than hostile-input paths, and both are contained by the out-of-thread watchdog rather than by a ceiling; removing them is what the recorded QuickJS-in-wasmtime fallback is for.
exit=0
```

---

# Screen 6 — `x image`

*Binary A · cwd `app` · capture `80-image-plan`*

```text
$ agentstack x image --toolset backend --harness claude-code
  Image  toolset backend for Claude Code → agentstack/backend:latest
  · from agentstack/sandbox:latest
  skill    greet       94b1a18e538a  /agentstack/home/.claude/skills/greet
  server   filesystem  3b986135ee09  /agentstack/servers/filesystem.json (carried)
  · no secrets are required at run time
  · posture SANDBOX / PROXIED · DIRECT ROUTE OPEN
  · posture belongs to the run, not the image — a bare `docker run` gets the container boundary only: no egress proxy, no allowlist, no run log
  · nothing has been written and Docker has not been contacted — agentstack x image --write builds it.
exit=0
```

---

# Gaps found — claim vs behaviour

Each is stated so it can be queued by name. None of these was fixed here; this
was an evidence pass.

## P8-G1 — ANSI colour is unconditional; `NO_COLOR` is ignored

`crates/cli/src` contains **zero** occurrences of `NO_COLOR` or `no_color`
(grepped at commit `9aef01e`), and `is_terminal` is used only to decide whether
to prompt, never to decide colour. Every capture on this page arrived with
escape sequences even though stdout was a file, not a terminal, and even with
`NO_COLOR=1` exported.

Consequences: piped output, CI logs and files carry escapes; the de-facto
standard (no-color.org) is unimplemented; and **every documentation sample of
these screens must have been passed through a stripper**, which is a silent
transformation no page currently discloses.

## P8-G2 — `use`'s dry run promises an apply that `--write` refuses

On an untrusted project, `agentstack use <toolset>` exits 0 and closes with
`Dry run. Re-run with --write to apply.` `agentstack use <toolset> --write` on
that same state exits **1** and applies nothing (screens 1a and 1b above).

The preview does print the `✗` blockers above that line, so this is not
silent — but the closing sentence is still a promise the write does not keep.
Same family as **G22** and **G24**, both of which were closed on the reading
"name the command that unblocks the state the user is now in". G24's closure
note records that a lane audit found it was "the LAST of its family"; this is
an instance that audit did not reach.

## P8-G3 — `apply`'s dry run has the same shape

On a trusted, locked project where every capability routes to the live lane and
no bridge is registered, `agentstack apply` exits 0 with
`0 targets would change. Re-run with --write to write.` and `apply --write`
exits **1** with `error: nothing was delivered`. Same family as P8-G2; the
counted fact ("0 targets would change") is honest, the instruction is not.

## P8-G4 — `use --write` reports activation on targets in a run that failed

The failing untrusted write closes with
`⚠ activated 'backend' on 4 targets (wrote 0); 3 targets BLOCKED: …` and then
`error: 3 targets blocked`, exit 1. Claiming activation "on 4 targets" while
writing zero files, in a run that exits nonzero, reads as a partial success
where there was none. The four counted targets are the ones with nothing to do.

## P8-G5 — `--unprotected` is unreachable from every non-interactive path

- `agentstack run <h> --unprotected --plan` → **exit 1**:
  `--plan needs a gated run mode — nothing was launched`.
- `agentstack run <h> --unprotected --prompt "…"` → **exit 1**:
  `--prompt needs the protected run — nothing was launched`.

So the documented escape hatch has no preview and no headless form; its
`HOST / ADVISORY` banner can only be reached from an interactive terminal.
`docs/howto/lock-down-a-run.md` describes `--unprotected` and its banner with
no mention of either restriction, and `plan/one-page-draft.md`'s four-posture
table presents it alongside three postures that both flags accept. Decide
whether the restriction is intended and document it, or widen the flags.

## P8-G6 — `workflow explain --json` does not carry the `role_details[]` the contract table promises

`docs/automation.md` (and its generated `automation.html`) lists
`agentstack x workflow list --json` **and** `workflow explain --json` under one
row of the `workflow-role-selection-v1` contract, described as *"per-entry
`role_details[]`"*.

Observed at `9aef01e`:

- `x workflow list --json` → `workflows[].role_details[]` ✓
- `x workflow explain --json` → a top-level **`roles[]`** array with the same
  per-role fields, and **no `role_details` key anywhere** (grepped the capture:
  0 occurrences).

A machine consumer implementing the documented contract against `explain`
finds nothing. Either the row should name the two different key paths, or
`explain` should emit `role_details`.

## P8-G7 — a manifest schema error names neither the file nor the fix

A manifest missing the top-level `version` key fails with
`error: manifest does not match the expected schema: missing field `version``
and exit 1 — no path, no line, no statement of what a valid header looks like.
The same shape appears for a `[servers.X]` table missing `type`. Low severity,
but it is the first error a hand-written manifest hits.

## P8-G8 — `x workflow list` answers for an untrusted bundle; `explain` refuses

On an untrusted project, `x workflow explain <name>` refuses (exit 1, "nothing
from an untrusted bundle normalizes or is invocable") while `x workflow list`
exits 0 and prints the workflow's name, its roles, and its declared ceilings —
correctly marked `TRUSTED false`. This looks deliberate (names and declared
numbers are manifest metadata, not normalized script facts) and is recorded
here as an observed asymmetry to confirm, not as a defect claim.

---

# Corrections to `plan/p8-scope.md`

The scope document's findings reproduced. Three of its recorded facts have
moved or were imprecise:

1. **`x up` exit code.** E7 records `x up … exit 0` on a trusted project. At
   `9aef01e` the same state exits **1** when no gateway bridge is registered,
   because commit `c5a24a1` ("up adopts apply's exit") landed after the scope
   was written. With a bridge registered it is exit 0. E7 is stale, not wrong
   for its date.
2. **E4 and E5 are reconstructions.** The `workflow run` and `workflow report`
   transcripts abbreviate every grant digest to eight characters plus an
   ellipsis; the binary prints all 64. E4 also drops the `▶ agent #3 [reduce]`
   line and the `· map outputs:` line, and collapses the result JSON to one
   line. E5 replaces the posture block with a bracketed description of it.
   Screens 5 above are the captures.
3. **E10's closing line.** The scope shows
   `` `agentstack x image --write` builds it. `` with backticks; the binary
   prints it without them.

---

# What is NOT verified

Stated plainly, because the standard here is that a reconstruction must never
be presented as a capture.

- **The `--unprotected` / `HOST / ADVISORY` screen.** Not captured. Both
  non-interactive doors refuse it (P8-G5), and a pty could not be allocated in
  this environment (`script -q` produced an empty file and exit 1 on two
  attempts). The banner's content is therefore **unverified** — believable from
  the flag's refusal messages, but nobody on this page has seen it.
- **Real-model workflow semantics.** Every workflow child on this page was a
  ten-line shell script, not a model. That is the right harness for governance
  claims — admission, grants, ceilings, taint, evidence — and it proves nothing
  about model behaviour or the performance bookends in
  `examples/workflow-acceptance/README.md`. That run stays manual.
- **macOS kernel containment.** The read-only workspace refusal in screen 3e is
  real, but Docker on macOS is a Linux VM: the kernel enforcing the bind is not
  this machine's kernel. No claim of macOS-native containment is supported by
  anything here.
- **Every `docs/` sample not named on this page.** This pass checked the
  `run` refusal quoted in `docs/howto/lock-down-a-run.md` (matches verbatim),
  its "`--plan` needs no Docker" claim (confirmed), and the
  `workflow-role-selection-v1` row in `docs/automation.md` (P8-G6). Other
  samples across `docs/` were **not re-checked** — believable, not verified;
  check them against the CLI before relying on them.
- **`agentstack x image --write`.** The plan screen is captured; a real image
  build was not run on this pass.
