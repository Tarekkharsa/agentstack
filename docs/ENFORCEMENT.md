<!-- BUILD INPUT for this page on https://tarekkharsa.github.io/agentstack/ —
     readers go to the site, contributors edit this file.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# AgentStack — Enforcement matrix

*Current as of agentstack 0.18.0.*

> Short codes below — `W2`, `W4`, `D3`, `D4`, `G9`, `Phase 1`–`Phase 4` — name
> internal development milestones, not public releases or CVE-style
> identifiers. They record *when* a behaviour landed relative to other work in
> `TODO.md`; nothing in this document depends on knowing them, and every claim
> beside them stands on its own.

This is the authoritative, code-grounded answer to one question: **for each
execution mode, what does AgentStack actually enforce, and by what mechanism?**
When any other document and this one disagree, this one is right — it is checked
against the source, not against intent.

Audience: anyone deciding what a mode actually guarantees.

**Contents**

- [Claim discipline](#claim-discipline)
- [What "trusted" does and does not mean](#what-trusted-does-and-does-not-mean)
- [Policy is authority, not isolation](#policy-is-authority-not-isolation)
- [The matrix](#the-matrix)
- [Per-cell notes](#per-cell-notes)
  - [Tools](#tools)
  - [Egress](#egress)
  - [Secrets](#secrets)
  - [Filesystem — write](#filesystem--write)
  - [Filesystem — read](#filesystem--read)
  - [Audit / recording](#audit--recording)
  - [Servers](#servers)
  - [Skills](#skills)
  - [Instructions](#instructions)
  - [Settings](#settings)
  - [Hooks](#hooks)
  - [Native extensions](#native-extensions)
  - [Workflows](#workflows)
  - [The protected run's frozen grant (the default `run`)](#the-protected-runs-frozen-grant-the-default-run)
  - [Images (`agentstack image`)](#images-agentstack-image)
  - [Trust-store mutation logging](#trust-store-mutation-logging)
  - [Intake detection (dropped files)](#intake-detection-dropped-files)
  - [Single-action activation (`agentstack yes`)](#single-action-activation-agentstack-yes)
  - [Review-card state on disk (snapshots, recognition, decisions)](#review-card-state-on-disk-snapshots-recognition-decisions)
- [Sharing, intake, and what a signature is worth](#sharing-intake-and-what-a-signature-is-worth)
  - [Bundle signatures — `share` / `receive`](#bundle-signatures--share--receive)
  - [Quarantine — where intake waits](#quarantine--where-intake-waits)
  - [Attribution — `license` and `origin` in the lock](#attribution--license-and-origin-in-the-lock)
- [Experimental `tools_execute`](#experimental-tools_execute)
- [See also](#see-also)

AgentStack intercepts an agent CLI on four independent lanes — one observes, three enforce:

![Four interception lanes: your agent CLI flows through the observe-only proxy to the Anthropic API, and through the enforcing gateway+mcp, guard, and egress interceptors to MCP servers, your filesystem, and the internet](interception-map.svg)

## Claim discipline

AgentStack **restricts destinations and records decisions; it cannot guarantee
that sensitive content never leaves through an allowed destination.** An enforced
egress allowlist blocks connections to hosts you did not approve — it does not
inspect payloads, and it permits traffic to every host you *did* approve,
including the model API itself. A prompt-injected agent can still exfiltrate
through any allowed channel. The honest claim is: *untrusted project
declarations are not auto-activated, and unapproved egress is blocked on the
enforced paths* — never "exfiltration is impossible."

Read every cell below with that ceiling in mind. "Enforced" means the disallowed
action is *prevented at runtime* (by the kernel, the container boundary, or the
proxy); it never means the allowed action is *safe*.

## What "trusted" does and does not mean

Trusting a project asserts exactly one thing: **the current manifest, local
overlay, and lockfile consent digest was approved for automatic loading on this
machine.** The lockfile separately pins resolved server definitions, skills,
and instructions; drift in those inputs fails verification. Detached
signatures attest to lockfile bytes but do not silently create local trust.

Trusted does **not** mean:

- **Safe to run unsandboxed.** Trust gates *whether* a bundle's servers spawn,
  its skills enter context, and its secrets resolve. It does not confine what a
  running agent then does — that is the job of policy and the sandbox, per the
  matrix below.
- **Vetted for correctness or intent.** `agentstack trust` summarizes the
  runtime surface—commands, HTTP contacts, secret refs, and skill pin status—so
  *you* can judge it. AgentStack verifies the consent digest and lock pins; it
  does not vouch for what the referenced code does.
- **Tamper-proof against a compromised host agent.** In host mode the agent CLI
  runs as you, so it can in principle reach the user-writable trust store under
  `~/.agentstack/` and self-trust a bundle. Only the sandbox removes this. The
  interactive consent probe is part of the same honest limit: `agentstack
  trust` treats a terminal on stdin as attended consent, and a same-user
  process that allocates a pseudo-terminal (`script`, `expect`, a `pty`
  wrapper) reads as interactive — no stronger than the store-file boundary
  above, and not claimed to be. What the gate does enforce is that headless
  callers (pipes, RPC servers) cannot grant without `--yes` plus the reviewed
  `--consented`. Every store mutation now leaves an identity-only
  event behind (see [Trust-store mutation
  logging](#trust-store-mutation-logging)), which makes silent self-trust
  harder to miss — but the log lives in the same user-writable directory, so
  it is evidence, not a defence.

Conversely, **untrusted project declarations are inert on automatic and
experimental execution paths**: the auto-project gateway does not spawn or
contact their MCP servers or resolve their secrets, and `tools_execute` refuses
to begin. Since the write gate reached all five capability kinds, an explicit
static `agentstack apply --write` is blocked too: it renders no server config,
no skills, no instruction fragments, no hooks and no extensions for a project
that is untrusted or drifted. What this still does *not* do is sandbox
arbitrary repository code or prevent a user from running it by hand — those are
separate authorization and execution paths, and a harness that reads bytes
already on disk starts them outside agentstack entirely.

## Policy is authority, not isolation

Policy decides *which* tools, hosts, secrets, and paths are permitted; it does
not decide *where* the process runs. **Policy is not a sandbox** — an allowed
tool can still have side effects, and an allowed host can still receive
sensitive data. Confinement is the job of `--sandbox` and `--lockdown`, per the
matrix below; the two compose but are never substitutes.

The shipped presets (`examples/policies/`) map to intent, not to a single
universal mode: use **developer** for daily work, **compatible** as a migration
step from an unmanaged setup, **locked-down** when a run needs confinement, and
**ci** as a runner-only floor. In every case the effective ruleset is the
machine ∩ project intersection, so a preset can only narrow what the machine
ceiling already allows.

## The matrix

Modes are columns; policy dimensions are rows. Legend:

- **enforced** — a runtime mechanism prevents the disallowed action; bypass means
  defeating the kernel, the container, or the proxy.
- **coarse** — a real check runs, but at coarser granularity than the policy can
  express (whole-workspace mount vs. per-path; host-only at write time vs. exact
  host:port at runtime; once-at-construction vs. per-call).
- **unsupported** — no code path on this mode consults the policy for this
  dimension; the dimension has **no effect** here. (Stated bluntly rather than
  softened to "advisory": for these cells no check happens at all, so there is
  nothing to bypass.)
- **cooperative** — a real per-call check runs, but only because the harness
  chooses to consult it (a pre-tool-use hook). Protects against an agent's
  *accidents*; a harness that ignores its own hook protocol, or a process the
  harness never routes through hooks, bypasses it entirely. Strictly weaker
  than **enforced** and never to be described as enforcement.

| Dimension | `host` | `gateway` | `lease` | `--sandbox` | `--lockdown` |
|---|---|---|---|---|---|
| **Tools** | unsupported | **enforced** | **enforced**◇ | **enforced**† | **enforced** |
| **Egress** | coarse | coarse | coarse | **enforced**\* | **enforced** |
| **Secrets** | **enforced** | **enforced** | **enforced** | **enforced**‡ | **enforced** |
| **Filesystem — write** | cooperative¶ | cooperative¶ | cooperative¶ | coarse | coarse |
| **Filesystem — read** | cooperative¶ | cooperative¶ | cooperative¶ | coarse | coarse |
| **Audit / recording** | unsupported | **enforced** | **enforced**◆ | **enforced**§ | **enforced** |
| **Native extensions** | unsupported‖ | unsupported‖ | n/a◈ | unsupported‖ | unsupported‖ |
| **Hooks** | unsupported⁂ | unsupported⁂ | n/a◈ | unsupported⁂ | unsupported⁂ |

◇ **the strongest tools cell, and fenced on top.** A lease is the `gateway`
column's dispatch path with one addition: the toolset. Every capability call
still goes through `Gateway::try_call`, which applies the compiled machine ∩
project tool policy and re-checks the consent digest before the upstream is
dialled — and on top of that, only the leased toolset's members are reachable.
With **no lease open, a project that declares toolsets serves control-plane
tools only**: the implicit union of everything declared is never served, because
capability exposure requires an explicit selection. Opening a lease exposes
exactly that toolset's members and nothing more; closing it returns the
connection to control-plane tools. What the fence does *not* do is constrain
what an allowed tool then does — see ◆.

◆ **every brokered MCP call recorded — not "every call recorded".** Each call
the lease dispatches lands in `calls.jsonl` (and a run's `events.jsonl` inside a
run) with digest-only arguments. That is evidence of the *request*: AgentStack
records what was asked of a server, and cannot observe what that server then
did internally. **Recording is not prevention** — a recorded call already
happened — and **an allowed destination can still exfiltrate**, exactly as the
claim-discipline section above states for every column. A lease also does not
make a call reproducible: only the pinned bytes in the lock do that.

Two further honest limits belong here rather than in a footnote nobody reads.
**A lease is process-scoped**: it belongs to the MCP process that opened it and
disappears with that process, so it is not a durable grant and cannot be
re-attached to. And the delivery claim is **"0 project artifacts for
gateway-delivered capabilities"**, never a bare "0 files": a project in this
lane still holds `.agentstack/agentstack.toml`, `agentstack.lock`, and —
whenever instructions are used — a managed region in an instruction file. Those
are rendered-lane artifacts and they are real.

◈ **not applicable — these kinds never enter this lane.** Native extensions and
hooks are executable capability kinds: their code runs inside (or around) the
harness process at full user permission. They are delivered by rendering files,
never over a lease, so there is no lease cell to label. The full consent
ceremony always applies to them, in a package or out of one, and no
compressed-consent path may ever cover them. Their real story is the ‖ and #
notes below.

\* **for proxied traffic only.** Plain `--sandbox` points `HTTPS_PROXY` at the
proxy but the container keeps an ordinary bridge network — a process that
ignores the proxy env can still dial out directly. The run is labelled
`SANDBOX / PROXIED · DIRECT ROUTE OPEN` for exactly this reason; only
`--lockdown` (no direct route, topological confinement) earns `ENFORCED`.
See the egress section below.

† **plain sandbox, for MCP traffic routed through the gateway.** A trusted run
renders one host-gateway entry into the harness config, so calls hit
`Gateway::try_call`. Plain `--sandbox` still has an open direct route: an agent
that independently reaches an egress-allowed upstream can bypass that gateway.
An untrusted bundle, a bundle with no proxied servers, or an incompatible
adapter can also be unrouted; those cases are surfaced at runtime. Under
`--lockdown`, D4 closes this qualification: the same frozen, pin-verified server
set drives gateway dispatch and the `gateway_only_hosts` egress fence; direct
connections to every declared HTTP MCP host are denied even when ordinary
egress policy allows them. If the gateway entry and native-config shadows
cannot be installed, lockdown refuses to start. Undeclared service aliases are
outside this exact declared-endpoint claim.

‡ **plain sandbox, for gateway-routed runs.** The host-side gateway resolves `${REF}` secrets
in its own memory and hands the container only the endpoint URL + a per-run
bearer token — resolved secret *values* never enter the container. A prior
`agentstack apply` that baked secrets into a project config is shadowed out.
(A run that isn't gateway-routed falls back to the coarse rendered-config path.)

§ **plain sandbox, for gateway-routed runs.** A gateway-routed run's own
`events.jsonl` gains a `ToolCall` per call (digest-only args) and a
`SecretAccess` per resolved ref (name only), alongside the lifecycle + egress
events it already held. Trust-store mutations and cost/tokens remain unrecorded.
See the Audit / recording section.

‖ **runtime is unsupported in every mode — this is a pre-delivery capability, not
a runtime one.** A native extension's code runs *inside the harness process at
full user permission*; no policy ceiling, gateway, egress fence, container, or
guard hook observes or constrains it once the harness loads it, so there is no
runtime cell to earn a stronger label. What agentstack governs happens entirely
*before* delivery: the source is content-pinned in `agentstack.lock`, an
untrusted or drifted project renders zero bytes, `apply` copies (never symlinks)
the pinned bytes into the harness's extension directory, and a protected `run`
re-verifies each delivered copy against its pin before launch. That pipeline is
provenance and content binding — which bytes, from where, reviewed by whom — not
runtime enforcement, and it is deliberately labelled as such. See the Native
extensions section.

⁂ **runtime is unsupported in every mode — a hook is a command the harness
itself runs at full user permission.** A `[hooks.*]` entry compiles into each
hook-capable harness's native hooks config; when the harness fires the event,
it executes the hook's command in its own process context — no policy ceiling,
gateway, egress fence, container, or guard observes or constrains that
execution, so there is no runtime cell to earn a stronger label. (The host
guard's own pre-tool-use hook is this same mechanism pointed at agentstack's
guard binary — that is why the filesystem rows above top out at
**cooperative**.) What agentstack governs happens before delivery, and it is a
*narrower* surface than the extensions pipeline: the hook's declaration — its
event, matcher, command line, and targets — is part of the manifest bytes, so
declaring or editing a hook re-gates trust review; rendering is fail-closed for
untrusted *and* stale-trust projects, at project and global scope alike
(`crates/cli/tests/red_team_hooks_trust_gate.rs`); and `doctor` reports
declared-vs-installed drift. But
when a hook's command names a local script, the *script file's bytes are not
content-pinned* — no lock entry digests them, so editing the script after
consent changes what runs without re-gating anything. That gap is documented
here deliberately rather than papered over. Strategy classification: hooks are
an executable capability kind alongside extensions — the full consent ceremony
always applies, and no compressed-consent path may ever cover them. See the
Hooks section.

Two of the five columns are execution modes for a *rendered* config: **host** is
`agentstack apply` + `agentstack run` (adapters write native config, the harness
runs on the bare machine and talks to upstream MCP servers directly).
**gateway** is the in-process broker (`agentstack mcp`, `connect`, code mode) —
every MCP call routes through `Gateway::try_call`. **lease** is that same broker
with a toolset selected for one MCP connection (`agentstack_lease_open`) — the
dynamic delivery lane, and the strongest column here. **--sandbox** and
**--lockdown** are `agentstack run --sandbox [--lockdown]`: the harness runs in a
Docker container behind the egress proxy.

The `host` column covers the **protected default** too. A plain
`agentstack run <cli>` now gates the launch fail-closed, but every one of those
gates runs *before* the harness starts, so it changes no cell here: pre-launch
gating is not runtime confinement, and the label `HOST / PROTECTED` says exactly
that much and no more.

**Which column a capability lands in is now routed, not chosen** (delivery flip,
2026-08-03; [`design/automatic-delivery.md`](design/automatic-delivery.md)). The
delivery planner sends **skills and MCP servers on an MCP-capable CLI** down the
dynamic lane by default, and **instructions, settings, hooks, extensions, and
every capability bound for a CLI without MCP** down the rendered lane. One
override, **Render locally** (`[delivery] render_locally`, per project or per
harness), forces the rendered lane where the lease would have worked; nothing
moves a capability the other way, because no channel would carry it.

None of that changes what any column *enforces* — the cells below are unchanged
by the flip, and a routing default is not an enforcement claim. What it changes
is which column an ordinary project is in: the **lease** column is now the
everyday one for skills and servers rather than an opt-in mode, so its honest
limits (◇, ◆, process scope, and the transparent-mode listing cost) are limits
most users now meet, not edge cases. Routing is also not activation: a
capability routed to the dynamic lane is served only once the bridge is
registered, the project is trusted at its current bytes, and a lease names a
toolset containing it.

## Per-cell notes

### Tools

- **host — unsupported.** `render_server()` and `plan_target_with_servers()`
  write the manifest's servers straight into the harness's native MCP config with
  the real command/URL; the harness then talks to upstream servers directly.
  `CompiledRuleset::tool_decision` is never called on this path.
  (`crates/adapters/src/render.rs`, `crates/cli/src/render/apply.rs`)
- **gateway — enforced.** Every call checks `tool_decision(server, tool)` before
  dispatch, and `namespaced_tools()` filters denied tools out of discovery too, so
  a denied tool is invisible *and* refused if called anyway. This is the single
  enforcement point. (`Gateway::try_call`, `crates/cli/src/gateway.rs`)
- **lease — enforced, plus a toolset fence.** Same enforcement point as
  `gateway` (nothing is duplicated: a lease *is* `Gateway::try_call` with a
  selected toolset), with the fence described at ◇ above. Two limits belong
  with the claim rather than under it:
  - **Lazy server start holds under the default compact mode, and is traded
    away deliberately in transparent mode.** Compact mode is the default: with
    no lease, the harness sees control-plane tools only, and a server is
    started or dialled on first tool use — so activating a toolset costs
    nothing until something is actually called. Transparent mode
    (`agentstack mcp --transparent`) advertises the upstream tools directly, and
    tools cannot be enumerated without asking the servers — so **transparent-mode
    tool listing starts every upstream in the fence**. That is the cost of the
    mode, chosen by whoever registered the bridge with `--transparent`; laziness
    is a property of compact mode and must not be claimed for both.
  - **A fence is not a sandbox.** Fencing decides *which* upstreams are
    reachable. It does not observe or constrain what a reached upstream does,
    and the Egress / Filesystem rows of this column are unchanged from
    `gateway` for exactly that reason.
- **sandbox — enforced for gateway-routed traffic.** A trusted run
  builds a host-side gateway (`Gateway::from_frozen` — hard trust gate: untrusted
  → empty → unrouted) and a token-gated HTTP MCP endpoint, then renders one
  gateway entry into the harness's user-scope config and shadows any direct
  project config. The container's MCP calls therefore reach `Gateway::try_call`,
  where `[policy.tools]` is enforced exactly as in gateway mode (denied at
  discovery *and* at call), and each call is recorded in the run's own
  `events.jsonl`. The container reaches the gateway directly through
  `host.docker.internal`. Both host-side listeners a `--sandbox` run stands up —
  that HTTP MCP endpoint and the run's egress proxy — bind the narrowest host
  interface the container can still reach that way, by the same rule and the
  same function the executor's relay uses (`relay_bind_address`): the private,
  non-routable docker0 bridge gateway on a native Linux daemon, or the host
  loopback on Docker Desktop — never a LAN-facing interface. `--lockdown` shares
  the endpoint bind, since its sidecar relay dials the same host address. The
  `0.0.0.0` wildcard is no longer a fallback. Where no narrow address is
  knowable or bindable — a Linux host whose docker0 gateway could not be
  determined at all (no daemon, no such network, no IPv4 gateway, or an address
  that would not parse); a Linux host that cannot bind the gateway it chose
  (Docker-Desktop-on-Linux, whose gateway lives in the VM), caught by an
  assignability probe before either listener starts; or any platform that is not
  linux/macOS/Windows — the run REFUSES to start and says why, naming the one
  opt-in. `AGENTSTACK_RELAY_BIND=<ip>` binds an explicit address and
  `AGENTSTACK_RELAY_BIND=0.0.0.0` accepts the LAN-reachable wildcard
  deliberately; the same variable governs the executor's relay, so the rule
  cannot differ between them. A wildcard bind is now always an operator's stated
  choice, never a consequence of an undetectable docker0.
  The bind is defence in depth in every case: the
  endpoint's per-run `X-Agentstack-Token` and the proxy's own per-run credential
  remain the authority, exactly as before. **Ceiling:** the ordinary bridge
  remains open; an agent that opens its own connection to an upstream host the
  egress policy allows bypasses the gateway. (`Gateway::from_frozen`,
  `crates/cli/src/gateway_http.rs`, `crates/cli/src/commands/sandbox.rs`
  `wire_sandbox_gateway` / `container_reachable_bind_ip`,
  `crates/egress/src/execution_relay.rs` `relay_bind_address`)
- **lockdown — enforced.** The container reaches the host gateway only through
  the egress sidecar's fixed-destination relay. The same frozen, pin-verified
  server set is handed to `Gateway::from_frozen` for dispatch and compiled into
  `gateway_only_hosts` for egress classification. That rule wins over an
  ordinary allow, so a direct connection to every normalized declared HTTP MCP
  host (all ports) is blocked while the relay remains the sole MCP route. stdio
  upstreams stay host-side. Literal-IP and non-TLS CONNECT targets are refused;
  partial, drifted, or unclassifiable server resolution fails the run; and an
  adapter whose gateway entry or native shadows cannot be installed is refused
  rather than given a rendered-config fallback. The relay is a fixed byte pipe;
  tool policy remains at the gateway. Precise ceiling: AgentStack fences the
  declared normalized endpoints, not every undeclared DNS alias the same service
  might operate. (`crates/cli/src/commands/sandbox.rs`,
  `crates/runtime/src/lockdown.rs`, `crates/egress/src/decide.rs`)

### Egress

- **host — coarse.** Write/spawn-time check only: for HTTP servers the declared
  URL host is extracted and `egress_decision(name, host, None)` is called with
  **port `None`** when the config is written. A host hidden behind an unresolved
  `${REF}` fails closed only if the server is egress-constrained. There is no
  runtime traffic filtering once the harness is running natively.
  (`crates/cli/src/render/apply.rs`)
- **gateway — coarse.** For HTTP upstreams the resolved host is checked once at
  construction (`egress_decision(name, host, None)`, port `None`); a constrained
  server whose host can't be determined is skipped. There is no per-call egress
  re-check, and stdio (child-process) upstreams get no egress check at all — their
  network access is unconstrained by AgentStack. (`crates/cli/src/gateway.rs`)
- **sandbox — enforced.** Every CONNECT is checked against `EgressGuard::decide`
  with the **real port** from the CONNECT line; resolved addresses must be global
  unicast (anti-SSRF, `netguard`); the TLS ClientHello SNI must equal the CONNECT
  host (anti-domain-fronting); a per-run token gates who may use the proxy at all.
  **Topology caveat:** `--sandbox` gives the container an ordinary bridge network
  with `HTTPS_PROXY` pointed at a host proxy — a container that *ignored*
  `HTTPS_PROXY` could still reach the open internet directly. Egress is enforced
  for traffic that goes through the proxy, not guaranteed the way `--lockdown` is.
  (`crates/egress/src/proxy.rs`, `crates/cli/src/commands/sandbox.rs`)
- **lockdown — enforced (topological).** The container is attached ONLY to an
  internal Docker network whose sole reachable peer is the egress-proxy sidecar;
  there is no host route, no internet, no DNS beyond it. Ignoring the proxy env
  reaches *nothing*. The sidecar runs the identical `ServerProxy` enforcement as
  `--sandbox`. This is strictly stronger: confinement is topological, not
  convention. (`crates/runtime/src/lockdown.rs`, `crates/egress/src/proxy.rs`)

### Secrets

- **host — enforced and fail-closed for a *denied* ref; overridable for an
  *unresolved* one.** `ScopedResolver::resolve` calls
  `secret_decision(server, name)` before returning any value. A ref denied by
  `[policy.secrets]` blocks the write with no escape hatch —
  `--allow-unresolved` forgives a missing secret, never a policy refusal. A ref
  that merely fails to resolve (no such secret on this machine, or a store that
  errored) also blocks the write by default, but under `--allow-unresolved` the
  renderer keeps the literal `${NAME}` and that placeholder is what reaches the
  config file. Once allowed, the concrete value is written into the native config
  file on disk — that on-disk exposure is a separate, accepted fact (ARCHITECTURE
  Layer 1), not a policy gap. Only the *refusal* leaves evidence here: a
  render-time resolution that succeeds emits no `SecretAccess`, because
  rendering happens outside any run and there is no run log to write one into.
  A run's recorded secret surface is therefore what the gateway resolved for
  that run, never what an earlier `apply` rendered into a config file.
  (`crates/cli/src/secret/mod.rs`, `crates/cli/src/render/hooks.rs`,
  `crates/cli/src/render/settings.rs`, `crates/adapters/src/render.rs`)
- **gateway — enforced, fail-closed.** A per-server `ScopedResolver` substitutes
  every `${REF}` through `secret_decision`; a ref outside `[policy.secrets]` fails
  to resolve, and the call is refused outright if any refs remain unresolved for
  that server. Same mechanism as host mode. (`crates/cli/src/gateway.rs`)
- **sandbox — enforced, for a gateway-routed run.** A trusted run
  routes MCP through the host-side gateway (`Gateway::from_frozen`), which resolves
  `${REF}`s fail-closed in its own memory via the same per-server `ScopedResolver`
  as gateway mode. Resolved secret *values* stay on the host — the container
  receives only the gateway's endpoint URL and a per-run bearer token. A prior
  `agentstack apply` that baked literal secrets into the project config is
  actively neutralized: `wire_sandbox_gateway` mounts an empty config over that
  path (shadowing it), so those bytes never reach the container either.
  **Fallback:** a run that is *not* gateway-routed — an untrusted bundle, a
  harness with no servers, or one that can't host an HTTP MCP entry — has no
  host-side resolution and, if a stale rendered config sits in the workspace, the
  container sees whatever was baked there. That path is coarse, as before.
  (`crates/cli/src/gateway.rs`, `crates/cli/src/commands/sandbox.rs`)
- **lockdown — enforced.** Secret resolution stays host-side as above, while
  D4 removes the fallback: a trusted run must install the token-bearing gateway
  entry and shadows, and an empty/untrusted run must install empty shadows. If
  either cannot be done, lockdown refuses to start. Resolved values therefore
  do not enter the container through AgentStack's MCP configuration path.
  (`crates/cli/src/gateway.rs`, `crates/cli/src/commands/sandbox.rs`)

### Filesystem — write

- **host / gateway — cooperative (¶), when the guard is installed.** No
  sandbox, no mount, no kernel path-scoping touches either path — `runs.rs`
  spawns the harness against the real filesystem, and stdio MCP children run
  with the ambient user's full permissions. What DOES run is the host guard:
  `agentstack guard install --write` wires `agentstack guard check` into each
  detected CLI's own pre-tool-use hook (Claude Code, Codex, Gemini, Cursor,
  Windsurf, Copilot CLI, Antigravity, OpenCode, Pi; VS Code agent mode reads
  the Claude-format user hooks). (VS Code's hook support is in Preview and
  may be disabled by an organization — coverage there is best-effort.) Per
  tool call it blocks: destructive
  commands (`rm -rf` outside the workspace, `git reset --hard`, `git clean
  -f`, disk writes, …), any access to `[policy.filesystem] deny` globs
  (machine ∪ project — a repo can only add), and writes outside
  the workspace + `[guard] allow_roots` + temp. That last confinement has an
  exact reach, worth naming.

  - **Shell writes** reach it on every wired CLI — a
  redirect, `rm`/`mv`/`cp`/`tee`, `sed -i` — because they arrive as commands
  and route through the same write-target check.

  - **File-tool writes** reach it
  on two signals. The tool NAME is the floor (`WRITERS`: `Write`, `Edit`,
  `MultiEdit`, `NotebookEdit`, `write_file`, `replace`, `edit_file`,
  `fs_write`, `create_file`, `str_replace_editor`, `replace_string_in_file`,
  `multi_replace_string_in_file`, `apply_patch`) — every name on it is a
  write, as before, and a name on it that arrives with NO readable target is
  refused rather than allowed: a write the guard cannot locate is a write it
  cannot confine.

  - **Codex's `apply_patch`** names its targets nowhere but inside
  its patch text, so the guard reads the documented envelope
  (`*** Begin Patch` … `*** Add File:` / `*** Update File:` /
  `*** Delete File:` / `*** Move to:` … `*** End Patch`, per the Codex
  parser's own constants) and puts EVERY path it finds through the identical
  write check a `Write` gets. One refused path refuses the whole patch.

  - **Beyond the list the PAYLOAD decides**, so a tool this build
  has never heard of is still confined when its call plainly intends a write:
  an edit structure (`old_string`/`new_string`, a patch, a list of edits), an
  explicit write mode or an `append`/`overwrite`/`create` flag, a body of
  content for the file it names, or an editor verb (`create`, `str_replace`,
  `insert`) in `command`. Key spellings are matched normalized, so the snake,
  camel and Pascal dialects all land.

  - **The residual is real and named:** a write
  whose call carries none of those signals — a path and nothing else, or
  content passed by handle — still degrades to the read path and gets the
  deny-glob check only. A path under a field name the guard does not read is
  now judged only when the tool's NAME is on `WRITERS` (there it fails closed
  and is refused); under an unknown name it is still not judged at all.

  - **The envelope reader is deliberately narrow:** it fires only when the whole
  argument is the envelope (first line `*** Begin Patch`, last line
  `*** End Patch`), so a patch smuggled around a shell command stays on the
  command path and keeps its destructive-command analysis — and `apply_patch`
  invoked through the SHELL (heredoc, or argv `["apply_patch", "<patch>"]`) is
  still analysed as a command, not as a patch. That degradation stays the safe
  default, chosen so an unfamiliar tool cannot wedge a harness.

  - **Cursor** is confined for shell writes and for
  nothing else: its surface offers no pre-write file hook, so the installer
  wires only `beforeShellExecution` and `beforeReadFile` and no Cursor file
  write is ever presented for a decision.

  - **`[guard.project_roots]`**
  scopes an extra root to one workspace ("sessions under `~/x` may also
  write `~/y`") — the grant lives in the MACHINE manifest, so a project can
  never widen its own write scope, and the guard denies shell writes to that
  manifest's directory precisely so this table can't be edited into
  allowlisting itself.

  - **Every denial is recorded to the audit log**
  (`host-guard` entries in `calls.jsonl`), and the two kinds stay tellable
  apart by their subject. A **rule** denial names the call it judged —
  `bash: …`, `read: …`, `write: …`, `other` — and carries the anchored
  workspace. The three fail-closed *system* refusals — an unreadable machine
  config, an unavailable machine policy, and an unreadable or oversized hook
  payload — are recorded under a synthetic subject
  (`system: machine-config-unreadable`, `system: machine-policy-unavailable`,
  `system: hook-payload-unreadable`) with no project, because what refused was
  the guard's own broken state rather than an evaluated rule, and no workspace
  had been anchored yet. The prefixes are machine-authored and payload content
  can only land after them, so no tool call can forge a system subject.
  Recording never gates the block: an unwritable audit log loses the evidence,
  never the denial.

  - **The ceiling is the legend's:** the harness must honor its own hook protocol — this catches
  accidents, not malice.

  - **Three CLIs are reported as NOT protected,** and the code
  keeps the two reasons apart because they are not the same promise.
  `NO_HOOK_SURFACE` is a fact about the CLI — Claude Desktop has no
  PreToolUse-style hook and Junie has only a static action allowlist, so there
  is nothing to ride and nothing to wait for. `NOT_WIRED` is a fact about
  agentstack: Kiro is unprotected because no guard hook has been built for it,
  not because none could be. Kiro's descriptor records its MCP config only, so
  this repo knows no hook file to install into and no entry shape uninstall
  could find again; a hook format guessed from outside the descriptors would be
  one the guard cannot honestly claim. Both cells are *unsupported* today; only the second one can change.

  - **Config unreadable** →
  the hook fails CLOSED; unrecognized payload shapes fail open (a guard
  that wedges the harness gets uninstalled, not fixed).
  (`crates/cli/src/guard.rs`, `crates/cli/src/commands/guard.rs`)
- **sandbox / lockdown — coarse.** The whole workspace is one bind mount, mounted
  `:ro` unless the effective write scope covers the workspace root
  (deny-by-default — the one dimension where absence means deny). A partial scope
  like `src/**` rounds *down* to read-only, since it's one all-or-nothing mount.
  The kernel enforces the `:ro` bind, not the harness. Coarse by definition:
  whole-workspace, not per-path. (`crates/cli/src/commands/sandbox.rs`,
  `CompiledRuleset::workspace_write_decision` in `crates/policy/src/ruleset.rs`)

### Filesystem — read

- **host / gateway — cooperative (¶), deny globs only.** The same hook guard
  checks every file-tool read and shell token against `[policy.filesystem]
  deny` (`.env`, key files, …) — so `cat .env`, `Read(.env)`, and `cp .env
  /tmp` are blocked in everyday host use. Reads are otherwise NOT confined
  to the workspace (confine-all-reads would break the harness itself; that
  is what the sandbox's mount boundary is for), and `FsRules.read` scopes
  are still never consulted on these paths.
  (`crates/cli/src/guard.rs`, `crates/cli/src/commands/guard.rs`)
- **sandbox / lockdown — coarse.** The whole workspace is visible inside the
  container and no *user* filesystem outside the mounted workspace directory
  is — so the workspace boundary itself is a real, kernel-level read scope.
  What else is mounted is agentstack's own generated content, never yours: the
  gateway config it renders for this run, and the empty shadows it lays over a
  stale project config, are bound **read-only** from a run-scoped `0700` temp
  directory that is removed when the run ends. One of those files carries the
  run's live gateway token, which is exactly why it is read-only and why it is
  not workspace content. No Docker socket, no `$AGENTSTACK_HOME`, no host
  home. But no finer mount
  is created from `[policy.filesystem] read`, so read globs narrower than the whole
  workspace are informational only. (`crates/cli/src/commands/sandbox.rs`,
  `crates/runtime/src/spec.rs`)

### Audit / recording

**Recorded is not prevented.** This whole section describes what is *written
down*, which is a different claim from what is *stopped*. An event proves a
check ran and what it decided; it never upgrades the cell that decided it. The
matrix rows above are the enforcement claim, and a family whose row says
`cooperative` or `coarse` keeps saying that no matter how completely its
decisions are logged. The two are set side by side deliberately, because
"we have a log of it" is the most tempting way to sound stronger than you are.

The lease column's recording claim is stated in exactly one form: **every
brokered MCP call recorded** — never "every call recorded". AgentStack records
what was asked of a server; it cannot observe that server's internal side
effects, and a call an agent makes by some route that never reaches
`Gateway::try_call` is not brokered and is therefore not in this log. Absence
from the log means "AgentStack did not broker it", which is not the same as
"it did not happen".

Which is which, per denial family:

| Denial family | Enforcement claim | Recorded? | Where |
|---|---|---|---|
| Gateway tool block | **enforced** (gateway/sandbox/lockdown) | yes | `calls.jsonl` + run `events.jsonl` (`ToolCall`, `outcome: denied`) |
| Egress refusal — sandbox proxy | **enforced** under `--lockdown`, `coarse`/proxied under plain `--sandbox` | yes | run `events.jsonl` (`Egress`, `allowed: false`) |
| Egress refusal — host path | **coarse** — a write-time check on the declared host, not a wire-level fence | yes, both halves — the render-time one **new in G9** | `calls.jsonl` (`tool: egress`) + run `events.jsonl` (`Egress`) when inside a run, for the gateway-build refusal and the one raised while *rendering* config (`apply` / `use` / `doctor`) alike. The render-time record names the server and the declared HOST, never the URL |
| Secret-scope refusal | **enforced** — the ref reaches no backing store | yes, **new in Phase 3** | `calls.jsonl` (`tool: secret`) + run `events.jsonl` (`SecretDenied`) |
| Filesystem guard | **cooperative** — the harness chose to ask | yes | `calls.jsonl` (`server: host-guard`, `run: None`) |
| Content-pin refusal | **enforced** — the server is dropped before it is spawned or dialled | yes, **new in Phase 4** | `calls.jsonl` (`tool: pin`) + run `events.jsonl` (`PinRejected`) |
| Trust-at-dispatch refusal | **enforced** — the call is refused before the upstream is dialled | yes, **new in W2** | `calls.jsonl` (`tool: trust`) + run `events.jsonl` (`TrustRefused`) |
| Toolset-fence refusal | **enforced** — the fenced gateway holds no upstream for the name, so nothing was spawned, dialled, or forwarded | yes, **new in W4** | `calls.jsonl` (`tool: fence`) + run `events.jsonl` (`FenceRefused`) |

Eight rows, seven families: `Family::Egress` refuses in two places that do not
enforce alike, so its row is split rather than averaged. Counting rows is not
counting families, and the paragraphs below number the families.

The toolset-fence row is the seventh family, added in W4 (leases). It fires
when a call names a server this project declares while no open toolset selects
it. Note the order: the fence had already emptied the gateway of that upstream,
so the record is *evidence* rather than the act — `mcp_server::fence_refusal`
turns what would otherwise be a bare "unknown tool" into a line that names the
toolset to open. One bound on it is deliberate: it records only for a server
the manifest **declares**, because otherwise any caller could write unbounded
rows into the audit log by inventing names. Inside a tracked run it writes the
same two-destination evidence its siblings write — the `calls.jsonl` line and a
`RunEvent::FenceRefused` mirror, which `agentstack report run <id>` renders in
its own **Fence refusals** section rather than among the tool calls, because a
refused call is not a call the run made.
(`crates/cli/src/seatbelt.rs` `Family::Fence`,
`crates/cli/src/mcp_server.rs` `fence_refusal`)

The trust-at-dispatch row is the sixth family, added in W2 (automatic
delivery). It fires when the project's consent digest stops matching the one a
*live* connection was authorized against — trust revoked, the manifest edited
out of band, or `agentstack.lock` replaced wholesale by a `git pull` or a
branch switch. Before W2 an already-spawned server stayed proxied until the
next lease, load, or session call happened to re-check, so a withdrawn yes left
a working path open; now every gateway dispatch recompares the digest, and any
uncertainty — an unreadable manifest, an inconclusive recompute — refuses.

Two honesty notes specific to it. First, what it empties is the **upstream
capability surface**: the leased servers' tools go away and `tools/list` stops
advertising them, while agentstack's own control-plane tools stay reachable on
the same connection, deliberately — a user whose project just went untrusted
has to be able to diagnose and fix it, and blinding them would turn a
fail-closed refusal into a dead end. Second, the comparison is recomputed on
every dispatch rather than cached: `git pull`, a manual edit, and a lock swap
all happen outside agentstack, so a cache here could only ever be a guess that
nothing moved.

The content-pin row is the fifth family, added in Phase 4. It differs from the
other four in what refused: nothing the user *authored* denied anything here,
the delivered bytes simply are not the bytes they reviewed — which is why it
has its own family and its own next step (review what changed, or re-pin
deliberately), rather than borrowing the tool block's. Under a sandboxed run
it only ever fires for a project that is already trusted: `Gateway::from_frozen`
carries the hard trust gate, so an unreviewed bundle is refused whole, earlier,
and never reaches per-server verification. The host, lease, and eager gateways
(`Gateway::from_manifest`, `from_manifest_lease`) carry no such gate — they
resolve and pin-verify every selected server for an untrusted project too, and
can emit this refusal for one. That is deliberate rather than an oversight:
those constructors are reached by naming the project, and on the eager path
`--manifest-dir` is itself the consent. What the refusal *means* is identical
on every path — the delivered bytes are not the bytes that were reviewed —
only the "already trusted" precondition is the sandboxed run's alone.

One honesty note specific to it: its refusal text is composed from lockfile and
manifest fragments, which are repository content and therefore hostile input
(invariant 7). It is control-character-stripped and length-bounded before it is
printed or recorded, so the reason in the log is deliberately lossy — a denial
the reader can trust to be a denial is worth more than a complete one.

The secret-scope row and the gateway half of the host-path egress row were
what Phase 3 gave events to: refusals that happened, printed once, and left
nothing behind. Adding those events changed **only** what is written — both
were already fail-closed refusals, both still are, and neither row's
enforcement claim moved as a result. The same is true of the
Phase 4 row: `Gateway::build` drops exactly the servers it dropped before, and
`refuse` still returns `()`. The host-path egress row in
particular stays `coarse` — recording a write-time decision does not make it a
runtime fence, and reading this table as though it did is the exact error the
paragraph above exists to prevent. G9 gave its render-time half the same two
destinations, on the same seam and with the same discipline: still `coarse`,
still fail-closed, and still only evidence that the check ran.

- **host — unsupported for MCP tool calls.** Native host-mode runs never call
  `calllog::record` for tool traffic because the harness talks to upstream MCP
  servers directly, bypassing AgentStack entirely. Audit of *calls* happens only
  if the harness is separately configured to route via the gateway (`agentstack
  mcp`). Since Phase 3 the host path does record its own
  **refusals** — secret-scope denials, the egress refusal raised while the
  gateway is being built, and, since G9, the write-time egress refusal raised
  while *rendering* config — which is why this cell is `unsupported` for the
  dimension (what the agent did) while those denials are nonetheless
  retrievable (what agentstack refused). The render-time one is the refusal
  most users actually meet: `render::apply` still pushes the reason string
  that `apply`, `use`, and `doctor` print as "blocked by policy", and now also
  files it through the same seatbelt recorders the gateway uses — the
  `calls.jsonl` line (`tool: egress`) and the `RunEvent::Egress` mirror. The
  record carries the server and the declared HOST, never the URL, because a
  declared URL can carry a credential in its userinfo, path, or query.
  Recording never gates it: an unwritable log loses the evidence, never the
  refusal.
  (`crates/cli/src/render/apply.rs` `record_egress_refusal`,
  `crates/cli/src/seatbelt.rs`, `crates/cli/src/gateway.rs`)
- **gateway — enforced.** `Gateway::try_call` logs every outcome (denied / ok /
  error) via `calllog::record` to `~/.agentstack/audit/calls.jsonl`. Only an
  argument *digest* is stored, never raw values or resolved secrets, and upstream
  error text is reduced to a fixed class so a malicious upstream can't write
  arbitrary bytes into the log. This is the most complete audit dimension.
  (`crates/cli/src/gateway.rs`, `crates/recorder/src/lib.rs`)
- **sandbox — enforced (for a gateway-routed run).** `RunLog::create`
  is mandatory and fails closed ("nothing trusted runs unobserved"). The run log
  captures container lifecycle (`SandboxStarted` / `SandboxExited`) and every
  egress decision — and, now that a trusted run's MCP traffic routes through the
  host-side gateway, every **tool call** (`ToolCall`: server, tool, outcome,
  argument *digest* only — never values) and every **secret reference** resolved
  (`SecretAccess`: ref *name* only). The gateway mirrors these into the run's own
  `events.jsonl` because it inherits the run id (`Gateway::from_frozen`), so
  `agentstack report run <id>` reads a self-contained record without the
  cross-project audit log. Trust-store mutations are recorded, but in their own
  machine-global stream rather than any run's log (see [Trust-store mutation
  logging](#trust-store-mutation-logging)); cost/tokens remain unrecorded.
  A run that isn't gateway-routed (untrusted bundle, or
  no servers) records only lifecycle + egress. (`crates/cli/src/gateway.rs`
  `log_call`, `crates/cli/src/commands/sandbox.rs`)
- **lockdown — enforced.** Run-log creation remains mandatory, and D4 makes the
  gateway the only route to declared MCP endpoints. Every possible declared
  MCP call therefore produces the same tool/secret evidence described above;
  an untrusted or serverless run has no MCP calls and records lifecycle plus
  egress. Cost/tokens remain the documented recorder gap; trust-store mutations
  are recorded outside the run log, as above.
  (`crates/cli/src/gateway.rs`, `crates/cli/src/commands/sandbox.rs`)

### Servers

- **pre-delivery — content-pinned by definition, trust-gated at launch.** A
  `[servers.*]` entry *is* its definition: transport, command, args, env, url.
  `agentstack.lock` pins that resolved definition's checksum
  (`LockedServer`), the manifest bytes are bound into the trust digest, and
  `doctor`'s `check_server_reproducibility` reports pin-vs-manifest drift.
  Editing a server's command line therefore re-gates trust review, with one
  named and now narrowed exception: a server tagged `owner = <adapter>` is
  refreshed from the owning app's own on-disk config by `apply --write`, and
  the re-pin depends on WHAT the refresh moved. An **environment-only**
  refresh — the motivating case, the Codex app rotating `node_repl` env
  values — is machine-derived from a config the owner already executes and
  authorizes no new executable content, so a project that was trusted
  immediately before the refresh has its trust **re-pinned** to the new
  digest instead of re-gated. A refresh that moved the **executable surface**
  does not get the carry: a stdio server's `command`/`args`, a remote
  server's `url` or the `headers` it presents there, or a change of transport
  `type` either way (`OwnedStatus::executable_moved`). There the manifest
  still records the fresh values, the re-pin is **withheld**, the project is
  left re-gated for the next command, and the run says which servers changed
  what they run or reach and that `agentstack trust` is owed. The owner's
  config is outside this project's consent digest — at project scope it is an
  in-repo file a `git pull` rewrites — so carrying trust across a new command
  line, or a new origin holding the auth header, would reach every harness
  with no review. A project that was already untrusted or drifted is left
  alone — pending review stays pending — and the re-pin digest comes from a
  pre-write snapshot with agentstack's own new bytes spliced in, never from a
  re-read of disk, so a hostile edit racing the write cannot be blessed by
  it. **The residual is disclosed, not hidden:** an env-value-only refresh
  still auto-repins, and env is executable-equivalent for an
  interpreter-launched server — `NODE_OPTIONS`, `LD_PRELOAD` and `PATH` all
  change what the same command line actually runs. (`crates/cli/src/render/owned.rs`
  `refresh_owned_servers` / `executable_surface_moved`,
  `crates/cli/src/commands/apply.rs`.)
  An untrusted or drifted project **spawns nothing through agentstack**:
  `session start`, the protected `run`, and the MCP server's auto-project gate
  all refuse, and `Gateway::from_frozen` refuses to build for a sandboxed run
  at all. **Its server config is not written either.** A `[servers.*]` entry is
  a command line (stdio) or an endpoint (http) that the harness spawns or dials
  *itself*, at its own startup, outside agentstack — so none of those
  launch-time gates is in the path, and the rendered file is the delivery. What
  the gate does, exactly (`render::apply::trust_refusal`, witnessed by
  `crates/cli/tests/red_team_servers_trust_gate.rs`): a project that is
  untrusted, or whose consent surface changed since it was trusted, renders
  **zero** server bytes — the destination file is left untouched, the refusal is
  recorded on the plan and re-enforced at the write choke point
  (`TargetPlan::write`), `apply --write` and `use --write` exit nonzero naming
  `agentstack trust`, and `--allow-unresolved` does not reach it (that flag
  forgives a missing secret, never a missing consent). The gate is on the
  content's provenance, not on the destination, so it is identical at project
  scope (`.mcp.json`) and global scope (`~/.claude.json`) — the global case
  being the sharper half, since a repository's command line otherwise lands
  where every project the user opens reads it. Two things are deliberately
  outside it, each because it is not the project's content: pruning entries
  agentstack already owns (the inert direction — a plan that manages nothing
  and only removes still writes), and the machine manifest at
  `$AGENTSTACK_HOME` — which `manifest::discover_project_base` refuses to
  discover as a project, so no `trust` command could ever satisfy a gate on it.
  `doctor` names `agentstack trust .` rather than `apply --write` when the gate
  is what holds delivery back, and `doctor --fix` refuses the same way instead
  of writing. **The honest limit:** a harness that reads *already-rendered*
  bytes on its own starts the server outside agentstack entirely, which is the
  accepted host-mode fact (ARCHITECTURE Layer 1) and not a promise this row
  makes; the gate governs what reaches the file, not what a file already on
  disk does.
  A stdio server's *executable* is a separate pin: repo-local commands and
  interpreter-script args are pinned as D3 `LockedExecutable` entries, which
  a protected `run` re-verifies before launch.
- **runtime — this is the tools row, not a separate one.** Once a server is
  spawned, what it may do is governed as tool calls: see [Tools](#tools) for
  what each mode enforces, and [Egress](#egress) for where it may talk. The
  honest limit is that pinning binds *what gets launched*, never what the
  launched process then does. A server fetched from a registry at a floating
  version is pinned at the definition agentstack resolved — the upstream
  package it names can still change beneath that definition unless the
  definition itself pins a version. (`crates/core/src/lock.rs` `LockedServer`,
  `crates/cli/src/commands/doctor.rs` `check_server_reproducibility`)

### Skills

- **pre-delivery — content-pinned, pin-gated AND trust-gated at
  materialization, trust-gated again at every path that serves the content.** A
  skill is instructional text an agent reads, not code agentstack executes.
  `agentstack.lock` pins each skill's bytes (`LockedSkill`, carrying its source
  — path, git, or library), and the manifest bytes are trust-bound. Two
  independent gates stand at materialization, and keeping them apart matters:
  * the **lock** gate — `verify::ensure_activatable` blocks a drifted or broken
    pin and deliberately lets an `Unpinned` entry through, because recording
    that first pin is itself the consenting act. Unchanged.
  * the **trust** gate — `render::skills::trust_refusal`, witnessed by
    `crates/cli/tests/red_team_skills_trust_gate.rs`. A project that is
    untrusted, or whose consent surface changed since it was trusted,
    materializes **zero** skill files: the refusal is recorded on the plan and
    re-enforced at the write choke point (`render::skills::materialize`), so a
    caller that ignores the field still cannot reach disk; the delivered set is
    left exactly as the human last approved it; and `agentstack use <toolset>
    --write` exits nonzero naming `agentstack trust`. The same choke point
    governs the additive materialization `agentstack add skill --write`
    performs, which is what makes it un-bypassable rather than a rule `use`
    happens to follow.
  Two things are deliberately outside the trust gate, each because it is not
  the project's content: removing skills agentstack already placed (the inert
  direction — deactivation and `x unrender` keep working untrusted), and the
  machine manifest at `$AGENTSTACK_HOME` — which
  `manifest::discover_project_base` refuses to discover as a project, so no
  `trust` command could ever satisfy a gate on it.
  Trust is then enforced *again* on every path that puts a skill in front of an
  agent: `session start` refuses an untrusted or drifted project outright and
  is stricter still (`ensure_session_startable` refuses `Unpinned` too, being
  the verb external UIs drive headlessly), the protected `run` refuses before
  launch, and the MCP server leaves an untrusted auto-project with
  control-plane tools only — `agentstack_list_loadable` returns skill *names*,
  and no skill body loads. Skills land on disk through `agentstack use
  <toolset> --write`, not `apply`, and pruning is scoped by the ownership
  ledger to what agentstack placed. (`crates/cli/src/render/skills.rs`,
  `crates/cli/src/verify.rs`, `crates/cli/src/session.rs`,
  `crates/cli/src/commands/locked.rs`, `crates/cli/src/mcp_server.rs`)
- **runtime — unsupported, and the honest reason matters.** A skill's content
  is prose the model reads. No mode inspects, filters, or contains it, because
  there is nothing to intercept: it is context, not a call. A reviewed skill
  can still contain text that steers a model badly — content pinning binds
  *which words you consented to*, never what the model does with them. That is
  why skill review is a human reading step, not a check.
  (`crates/core/src/lock.rs` `LockedSkill`)

### Instructions

- **pre-delivery — content-pinned per fragment, pin-gated, decision-gated AND
  trust-gated, compiled into managed regions.** Each `[instructions.*]` fragment
  is a local file pinned by the SHA-256 of its raw bytes (`LockedInstruction`),
  and `doctor`'s `check_instruction_reproducibility` reports drift between pin
  and file. Three independent gates stand before the region is written, and
  keeping them apart matters:
  * the **lock** gate — every readable declared fragment must still match its
    pin, and an unpinned fragment deliberately passes, because recording that
    first pin is itself the consenting act. Unchanged.
  * the **decision** gate — compilation honours the standing re-gate answers
    per fragment: `blocked` excludes the fragment, `keep-pinned` compiles the
    approved snapshot instead of the live file and excludes the fragment if
    that snapshot cannot be re-verified.
  * the **trust** gate — `render::instructions::trust_refusal`, witnessed by
    `crates/cli/tests/red_team_instructions_trust_gate.rs`. A fragment is not
    executable, and that is exactly why it is gated rather than excused: its
    bytes go into the managed region every harness reads at its own startup,
    straight into a model's context, with no agentstack process in the path and
    nothing to intercept at run time (see **runtime** below). A project that is
    untrusted, or whose consent surface changed since it was trusted, compiles
    **zero** of its own fragment bytes: the refusal is recorded on the plan and
    re-enforced at the write choke point (`render::instructions::InstrPlan::
    write`), so a caller that ignores the field still cannot reach disk; an
    existing region is left exactly as the human last approved it rather than
    emptied; and `apply --write` and `agentstack x instructions --write` both
    exit nonzero naming `agentstack trust .`. The gate is on the content's
    provenance, not on the destination, so it is identical at project scope
    (`./CLAUDE.md`) and global scope (`~/.claude/CLAUDE.md`).
  A package's instruction members are gated **as the project's content**: their
  pins live in the project's `agentstack.lock` and they compile into the same
  region, indistinguishable to the model from prose the repo wrote by hand, so
  moving instructions behind a package buys no exemption. Four things are
  deliberately outside the trust gate, each because it is not the project's
  content: emptying a managed region (the inert direction — `unrender` /
  `uninstall` keep working untrusted), machine-layer fragments (the next
  bullet), the machine manifest at `$AGENTSTACK_HOME` — which
  `manifest::discover_project_base` refuses to discover as a project, so no
  `trust` command could ever satisfy a gate on it — and the pre-command trust
  state a self-authoring command is judged against (`render::PriorTrust`), which
  is what lets `lock --write` and `upgrade` render the bytes they were just told
  to pin instead of refusing their own delivery. Compilation writes only into
  agentstack's managed region of `CLAUDE.md` / `AGENTS.md`, leaving hand-written
  content outside that region untouched and restorable.
  (`crates/cli/src/commands/apply.rs`,
  `crates/cli/src/render/instructions.rs`)
- **Layer scope — machine fragments are not repo content.** User/global-layer
  fragments are deliberately *not* pinned: they are yours, not the project's,
  and binding them into a project's consent digest would make your own
  machine's notes re-gate every repo. Project fragments are pinned; machine
  fragments are trusted by ownership.
- **runtime — unsupported, same reason as skills.** Instructions are text in
  an agent's context. Nothing intercepts them at run time, and a trusted
  fragment that says something unwise is still executed by the model's
  judgement, not agentstack's. (`crates/core/src/lock.rs` `LockedInstruction`,
  `crates/cli/src/commands/doctor.rs` `check_instruction_reproducibility`)

### Settings

- **pre-delivery — pinned per owned key, and drift-probed.** A `[settings.*]`
  value is merged into a named CLI's own configuration file, and
  `permissions.defaultMode` lives there, so a settings value is
  security-relevant. Each **top-level key** the manifest declares is pinned
  separately in `agentstack.lock` as a `[[setting]]` row carrying
  `target`, `key` and a SHA-256 checksum, and `doctor`'s Settings section
  reports drift against it. (`crates/core/src/lock.rs` `LockedSetting`,
  `crates/cli/src/commands/lock.rs` `record_setting_pins`,
  `crates/cli/src/commands/doctor.rs` `check_setting_pins`, witnessed by
  `crates/cli/tests/settings_pinning.rs`.)
- **The grain is the key, because the key is the unit AgentStack owns.**
  `render/settings.rs` merges one top-level key at a time, replaces it
  wholesale, prunes one it used to own, and leaves every other byte of the
  harness's file untouched. Pinning at that same grain is what lets a probe name
  *which* key moved, and what makes the coverage boundary structural rather than
  a promise: a key AgentStack does not declare has no row in the lock and can
  never be read as drift. **Your own unrelated edits to `settings.json` are not
  drift and are never reported as such.**
- **What the checksum covers: the value as DECLARED, `${REF}`s unresolved** —
  the same rule `LockedServer` follows. A resolved value is machine-specific
  (one committed lockfile would disagree with itself across two developers'
  machines) and can contain a secret (which must never reach a committed file).
  So "the delivered bytes are the reviewed bytes" is a chain of two legs, and
  `doctor` reports them separately because they have different fixes:
  **declaration ↔ pin** (the declared value still digests to what the lock
  records; fix: `agentstack lock --write`), and **declaration ↔ disk**
  (re-merging that key into the live file would change nothing; fix:
  `agentstack apply --write`). The clean line — "N keys in <path> match
  agentstack.lock" — is printed only when both hold.
- **Re-gate on change.** Settings live in the manifest, and manifest bytes are
  bound into the trust digest, so editing a `[settings.*]` value re-gates
  consent exactly like any other manifest edit; the consent card discloses each
  `[settings.<id>]` block by its canonical, key-sorted identity, so a changed
  value reads as `~ changed` while a mere re-ordering of the same keys does not
  (`crates/cli/src/commands/trust.rs` `settings_identity`). The pin now moves
  with it: a re-lock after a settings edit changes the lock bytes, which are
  consent material too. The pinning act deposits the canonical bytes it hashed
  into the content store (`Store::pin_settings_key`), so a re-gate can show
  which lines of a value moved rather than only that it moved.
- **delivery — NOT a fail-closed gate, deliberately.** Unlike an unpinned skill,
  instruction, extension or workflow, an unpinned or drifted settings key does
  **not** refuse a render, and `doctor` reports it as a warning rather than an
  error. Settings are inert configuration merged into a file the *harness* owns;
  refusing there would leave a user's own harness config half-written, for a
  class of change the trust gate already re-gates through the manifest bytes.
  What you get is a named finding and a named fix, not a stop.
- **runtime — unsupported, and this is the honest limit.** A settings value
  reaches the harness's own configuration file and the harness reads it
  directly. Nothing intercepts that at run time: if a `permissions` block is
  edited on disk after consent, the harness obeys the edited file until the next
  `apply --write` puts the declared value back. AgentStack **detects** that
  divergence and names the fix; it does not prevent it.
- **Backward compatibility.** A lockfile written before settings pins existed
  carries no `[[setting]]` rows. It keeps working: the section degrades to the
  pre-pin behaviour (the disk leg alone), reports the unpinned state as a
  warning naming `agentstack lock --write`, and never errors. The next
  `lock --write` backfills; there is no migration step. An empty pin list
  serializes to nothing, so a project that declares no settings keeps a
  byte-identical lockfile and is not re-gated by the arrival of the pin kind.
  (`crates/core/src/manifest/model.rs`, `crates/core/src/lock.rs`)

### Hooks

- **host / gateway / sandbox / lockdown — unsupported (runtime).** A
  `[hooks.*]` entry is a command the harness runs *in its own process context
  at full user permission* whenever the declared lifecycle event fires
  (`PreToolUse`, `SessionStart`, …). No runtime mode consults policy for it:
  it is not a tool call, so the gateway never sees it; the egress fence and
  the sandbox container never single it out from the harness that spawned it;
  and the host guard cannot referee it — the guard *is itself* a hook riding
  this exact mechanism. Every runtime cell is `unsupported`, and honestly so.
  (`crates/cli/src/render/hooks.rs`)
- **pre-delivery — declaration-bound and trust-gated, but NOT content-pinned.**
  The governed surface is the declaration: a hook's event, matcher, command
  line, args, timeout, and targets live in the manifest, whose bytes are
  bound into the trust digest — adding a hook or editing its command line
  re-gates trust review, and rendering into a harness's native hooks config is
  fail-closed for untrusted or drifted projects. What that gate does, exactly
  (`render::hooks::trust_refusal`, witnessed by
  `crates/cli/tests/red_team_hooks_trust_gate.rs`): a project that is untrusted,
  or whose consent surface changed since it was trusted, renders **zero** hook
  bytes — the destination file is left untouched, `apply --write` exits nonzero
  naming `agentstack trust`, and `--allow-unresolved` does not reach it (that
  flag forgives a missing secret, never a missing consent). The gate is on the
  content's provenance, not on the destination, so it is identical at project
  scope (`.claude/settings.json`) and global scope (`~/.claude/settings.json`).
  Three things are deliberately outside it, each because it is not the
  project's content: pruning hooks agentstack already owns (the inert
  direction), the machine layer's own guard hook, and the machine manifest at
  `$AGENTSTACK_HOME` — which `manifest::discover_project_base` refuses to
  discover as a project, so no `trust` command could ever satisfy a gate on it.
  `doctor`'s Hooks section reports declared-vs-installed render drift, and
  names `agentstack trust .` rather than `apply --write` when the gate is what
  holds delivery back. **The honest limit:** unlike a
  native extension, whose source tree is digest-pinned in `agentstack.lock`, a
  hook's command typically *names* a script whose file bytes have no lock
  entry — editing that script after consent changes what the harness executes
  without re-gating anything. What is bound is *which command line runs*, not
  *what the named file contains*. Consent policy follows from this: hooks are
  an executable capability kind alongside extensions and always get the full
  consent ceremony — never a compressed path. (`crates/cli/src/render/hooks.rs`,
  `crates/core/src/manifest/model.rs` `Hook`)

### Native extensions

- **host / gateway / sandbox / lockdown — unsupported (runtime).** A native
  extension (pi `.ts`, OpenCode `.js`) is executable code the harness loads and
  runs *in its own process at full user permission*. No runtime mode consults
  policy for it: the gateway never sees it, the egress fence and the sandbox
  container never contain it, and the host guard's pre-tool-use hook never
  intercepts it — it is not a tool call. Every runtime cell is `unsupported`,
  and honestly so. (`crates/cli/src/render/extensions.rs`)
- **pre-delivery — content-pinned, trust-gated, copy-rendered, then re-verified
  under the protected run.** This is the entire governed surface, and it runs before
  the harness ever loads a byte. The source is pinned in `agentstack.lock` with
  the strict integrity-root digest (symlinks rejected, `.git` included), so any
  change re-gates trust review. `apply` renders fail-closed: an untrusted or
  drifted project writes zero extension bytes, and only lock-matching sources
  are **copied** (never symlinked) into the harness's extension directory, so
  the delivered bytes are the reviewed bytes. Two things sit outside that gate,
  each because it is not a project's content: pruning artifacts agentstack
  already owns (the inert direction), and the machine manifest at
  `$AGENTSTACK_HOME` — which `manifest::discover_project_base` refuses to
  discover as a project, so no `trust` command could ever satisfy a gate on it.
  Same exemption as hooks, and witnessed by
  `crates/cli/tests/red_team_extensions_trust_gate.rs`. An ownership ledger scopes pruning
  to what agentstack placed and hard-excludes the guard's `agentstack-guard*`
  artifacts. Under a protected `run`, the `rendered-verify` gate re-digests each
  delivered copy against its pin before launch, refusing on drift and naming the
  extension. All of this is provenance and content binding — not runtime
  enforcement. (`crates/cli/src/render/extensions.rs` `render` / `verify_rendered`,
  `crates/cli/src/commands/locked.rs`, `agentstack_core::digest::integrity_root_digest`)

### Workflows

- **pre-delivery — pinned, probed, and witnessed.** A `[workflows.*]` entry is
  orchestration script bytes plus a declared role set. `agentstack.lock` pins
  both (`LockedWorkflow`: script checksum and `roles`, stored sorted and
  de-duplicated so a role-set change is drift even when the bytes are
  identical), which means widening a workflow's roles re-gates trust exactly
  like editing its code. `doctor` probes it from two sides —
  `check_workflow_reproducibility` for pin-vs-disk drift and
  `check_workflow_ceilings` for a declaration that exceeds machine policy —
  and the invariant has a named witness
  (`workflow_drift_and_roles_widening_block_trust_until_relocked`).
- **admission — the grant is frozen before anything runs.** A workflow's own
  grant is constructed once, re-asserting `meta.roles ⊆ admitted roles` and
  re-clamping ceilings, and every child step is spawned under a grant derived
  from it. No step can widen what the workflow was admitted with, and machine
  policy remains the ceiling over the whole tree.
- **runtime — NOT enforced by the workflow; each step gets its own posture's
  enforcement.** This is the sentence that matters. "Governed workflow" does
  not mean uniform containment: a step that runs in host mode is
  cooperative-guard-only, and only a sandbox or lockdown step gets kernel and
  egress fences. The report labels each step with its posture slug rather than
  letting the workflow's name imply the strongest one.
- **Step outputs are model output — untrusted data.** Results flow into later
  steps' prompts by design, so a prompt-injected step can mislead its
  successors. It cannot escalate: roles are a closed, pre-reviewed set and the
  ceiling is frozen at admission. Can mislead, cannot widen — that is the
  honest boundary.
- **Not enforced: token and cost accounting.** `budget` meters agent count and
  wall clock, but the two are enforced in different places and the difference
  matters. Agent count is the engine's: it counts spawns against the grant and
  refuses past it. `max_wall_seconds` is **inert inside the engine** — a number
  the engine surfaces to the script, never a clock it reads, because the engine
  is deliberately clock-free so a run stays replayable. Wall time is enforced by
  the CLI instead: the drive loop checks the deadline at every batch boundary
  and fails the run, and an out-of-thread watchdog force-exits past the ceiling
  plus a grace. So the ceiling is real; it just is not the engine that holds it,
  and a workflow driven by anything other than that loop gets no wall
  enforcement at all. Tokens the engine cannot observe uniformly
  across harnesses, and the recorder's cost dimension is still unwired.
  (`crates/core/src/lock.rs` `LockedWorkflow`, `crates/workflow/src/lib.rs`,
  `crates/cli/src/commands/workflow.rs` `run_value` / `spawn_watchdog`,
  `crates/cli/src/commands/doctor.rs` `check_workflow_reproducibility` /
  `check_workflow_ceilings`)

<a id="the-locked-runs-frozen-grant-run---locked"></a>
### The protected run's frozen grant (the default `run`)

- **When it applies.** A bare `agentstack run <cli>` takes this path;
  `--locked` names it explicitly and still works; `--unprotected` opts out to an
  ungated `HOST / ADVISORY` run with none of the gates below. Only the *default*
  moved — every claim in this section is byte-for-byte the claim it was, and an
  `--unprotected` run earns none of it.
- **What is enforced.** The launch is gated fail-closed *before* the harness
  starts: enforced trust, strict lock verification (including the D3
  executable pins — pin derivation and verification share one classifier and
  both anchor at the project root, so record and verify can never disagree),
  `rendered-verify`, and policy admission against the machine ceiling. The
  run's MCP surface is then **frozen**: the compiled machine ∩ project
  ruleset and the `${REF}`-only server set are sealed (HMAC, machine-local
  key) into a private run artifact, and the launch-scoped bridge
  (`agentstack mcp --grant`) serves exactly that — refusing on a failed MAC,
  consent drift, lost trust, version skew, or a machine ceiling that changed
  since freeze, and refusing every mutating/secret-resolving control-plane
  tool (lease transitions, `session_start`, manifest editors) for the run's
  duration. It never falls back to disk re-derivation.
  (`crates/cli/src/commands/locked.rs`, `crates/cli/src/grant.rs`,
  `crates/cli/src/mcp_server.rs`; asserted end-to-end in
  `examples/projects/locked-run/`.)
- **What is NOT claimed.** No kernel isolation — the harness runs as you on
  the host; `--lockdown` is the fence. The sealing key is readable by the
  same user, so the artifact MAC stops cross-machine replay, on-disk
  tampering, and forgery by anything that cannot read the key file — not a
  same-user unconfined process (which already runs at full permission here;
  the manifest cross-check hardening is staged for the confined tiers).
  Ambient **user/global-scope** MCP entries are named in an honest
  content-derived warning, not neutralized (parking a shared global config
  that harness apps rewrite mid-run would risk clobbering user state). A
  machine-policy tightening mid-run takes effect at the next run, matching
  the in-process gateway's snapshot-at-start semantics.

### Images (`agentstack image`)

**What ships:** one toolset and its pinned members composed into a container
image the user builds locally and runs themselves
([`design/packaging.md`](design/packaging.md)). Skill bodies are copied out of
the content store by the digest `agentstack.lock` records — the same
pinned-serving rule the MCP and rendered lanes follow — and laid down in the
harness's own skills directory inside the image. Server *definitions* travel
verbatim under `/agentstack/servers/`, `${REF}` placeholders intact. Nothing is
pushed, tagged remotely, signed, or registered, and nothing phones home. The
build refuses fail-closed on an unpinned member, an unverifiable store deposit,
a server the frozen resolution rejects, or a project that is not trusted at its
current bytes.

**The posture label, and its exact scope.** The artifact carries the shipped
`Posture::Sandbox` label — `SANDBOX / PROXIED · DIRECT ROUTE OPEN` — and that
label describes what the image is *prepared for*, never what a run enforces.
**Posture is a property of the run.** Every mechanism the `--sandbox` column
above claims is supplied by whoever starts the container: the proxy, the
allowlist, the run log, the gateway. Consequently:

- A **bare `docker run <tag>`** earns the container boundary and nothing else —
  no egress proxy, no `HTTPS_PROXY`, no allowlist, no flight recorder, no
  gateway. It is *not* the `--sandbox` column, and no surface says it is.
- Run through `AGENTSTACK_SANDBOX_IMAGE=<tag> agentstack run … --sandbox`, it
  is exactly the `--sandbox` column with every qualification in this document
  intact, including `*` (proxied only; the direct route stays open).
- `--lockdown` is stronger and is **deliberately not claimed by the artifact**:
  topological confinement comes from the internal network and the egress
  sidecar, neither of which an image contains. The same image run under
  `--lockdown` earns that column; the image itself never advertises it.

**What it is not.** Packaging adds **no enforcement of any kind**. It changes
where reviewed bytes are, not what a process holding them may do. It is also
not a reproducibility claim beyond AgentStack's own layer: the members are
content-addressed and identical across machines, but a Docker build is not
bit-reproducible (layer metadata varies per build) and the `FROM` base is a
floating tag unless the user passes `--from` a digest. And no secret is ever
baked — the build constructs no resolver at all, the image carries only the
`${REF}` *names* it will require, and a start-up guard refuses to launch the
harness until those names are present in the run's own environment.

### Trust-store mutation logging

**What ships:** every mutation of the machine trust store appends one
identity-only line to `~/.agentstack/audit/trust.jsonl` — timestamp, action
(`grant`, `regrant`, `repin`, `revoke`, `decide`, `undecide`), the store's own
project key, and the consent digest pinned, removed, or already stood on. Never
the manifest bytes, never the reviewed surface. A standing re-gate answer
(`keep-pinned` or `blocked`) is written onto the same project entry in the same
store file, so it appends a line too — `decide` when one is recorded,
`undecide` when one is withdrawn. The split is what the identity-only rule
costs: WHICH item was answered and WHAT the answer was are consent content and
stay out of the log, so the action name is the only place the direction of the
change can live. A call that changes nothing — re-affirming an identical
answer, or clearing one nobody gave — writes nothing and records nothing. The
log now answers both "what was consented to, and when" and "what changed in
`trust.json`". The append happens inside the store lock and only after the store
write succeeded, so log order is store order and every event describes a
mutation that actually happened. `repin` is recorded distinctly from
`regrant` because no human consented to it. The file is created `0600` and is
never rotated: consent metrics count over the full history.

**What it is not:** the append is best-effort — if it fails, the grant still
succeeds and the event is simply lost, because recording must never gate
consent. The log is append-only by convention, **not tamper-evident**, and it
sits under the same user-writable `~/.agentstack/` as the trust store itself.
A compromised host-mode agent that can self-trust can also delete or rewrite
this log. It raises the cost of unnoticed self-trust; it does not prevent it.
Only sandbox mode removes the underlying risk.

### Intake detection (dropped files)

**What ships:** files sitting in a project's own `skills/` and `instructions/`
directories that no manifest entry declares are noticed at command time by
`status`, `doctor`, `use`, `lock`, and `adopt`, and offered for adoption with a
preview. Undeclared content is **inert**, and that is a property of the
existing design rather than a new check: nothing enumerates those directories,
so undeclared content is never resolved, pinned, materialized, or placed in an
agent's context. Detection reads it and reports names, paths, and a one-line
summary; adoption writes a manifest entry and nothing else — the lock still has
to pin the bytes and the trust gate still has to pass before anything is
delivered. Every byte read is treated as hostile input (invariant 7): bounded
reads, bounded entry counts, symlinks refused rather than followed (the
directory entry *and* the file whose bytes would be read), names validated
before they can become manifest keys, and all displayed text passed through the
shared terminal sanitizer. A dropped file whose name a manifest entry already
uses is reported and **not** adopted: replacing a pinned declaration is a
different act from bringing in something new, and it never happens behind a
preview that presents it as an addition.

Each item is classified by provenance. Content already recorded as having
arrived through `receive` or `add from` is settled first, before git is
consulted at all: those bytes are a stranger's work whatever git would say, and
adopting them lands them untracked, which is exactly how they would otherwise
read as the user's own. After that, inside a git work tree tracking alone
decides — untracked is the user's own work, tracked came with the project —
read through a single hardened `git ls-files -z` over the intake directories.
Outside a work tree there is no tracking signal and no fallback: every item is
classified as arrived, for the stated reason "no git history to attest who
authored this". The classification is shown to the user and gates only
*compression* of the first-time adoption path; it never gates adoption itself,
so a project outside git simply takes the full staged review.

**What it is not:** provenance is a heuristic about origin, **not an integrity
claim**. Untracked-in-git means git has not seen the file, which anything with
write access to the working tree can arrange. Modification time is consulted
*nowhere* — not for tracked files, because git rewrites it on every checkout,
and not outside a work tree either, because `touch` is free to any process with
filesystem access. That is why the absence of git yields no compressed path
rather than a timestamp comparison: a signal an attacker can forge outright is
worse than no signal. The signals that do remain still do not survive an
attacker who already has local write access, and none of them is a substitute
for reading what you are adopting. Detection is also not a monitor: it runs
when you run a command, so content dropped and removed between commands is
never seen.

### Single-action activation (`agentstack yes`)

**What ships:** for locally-authored dropped files with no name collision, one
command performs declare → lock → trust → render behind one review and one
confirmation. **The collapse is presentation, not semantics.** It calls the
same functions the explicit sequence calls, and the grant goes through the one
`grant_gated` path `agentstack trust` uses — same surface, same digest, same
recorded `trust.jsonl` events in the same order. That parity is a witness test
(`crates/cli/tests/funnel_activation.rs`), not a claim. The review shows
everything the separate steps show — including the real activation dry run,
which is the actual `use` code path with writing off, so the preview cannot
drift from what follows it — plus, for each item, the provenance line saying
why it qualified. Declining restores the manifest and lockfile to their
previous bytes, so a refusal leaves the project as it was.

The command requires a terminal. It is a review a human reads and answers;
headless callers keep the explicit path, where `--consented` binds the
acknowledgement to previewed bytes (§7.2). Content that fails provenance or
collides with an existing declaration is not filtered out downstream — it is
never in the set the compressed path acts on, and the user is told which
command reviews it properly.

**What it is not:** this is **first-time adoption only**. Re-consent to
*changed* content is not compressed and stays on the explicit path until the
review card can render a real diff of what changed — compressing a re-gate
before the change is visible would be worse than the friction it removes. It
does not widen what a yes grants: the same bytes are consented to, in the same
gate, with the same effect. It is also not a review the tool performs on your
behalf — nothing here inspects what a skill's text tells a model to do, for the
reason stated under **Skills** above.

### Review-card state on disk (snapshots, recognition, decisions)

**What ships:** the review card keeps three pieces of state so a re-gate can
show *what changed* rather than only *that something changed*, and so repeated
review of identical content gets shorter.

- **The content snapshot store**, `~/.agentstack/store/content/<sha256>/`. The
  bytes a pin covers, deposited at pin time as part of producing the pin
  itself: the checksum a lockfile entry carries comes from the depositing
  function and from nowhere else — `Store::pin` for skills,
  `Store::pin_instruction` for instruction fragments,
  `Store::pin_server_definition` for server definitions,
  `Store::pin_integrity_root` for extensions and workflows, and
  `Store::pin_blueprint` for a workflow's approved blueprint. The
  integrity-root kinds deposit only while the pinned byte set stays within the
  store's deposit ceiling (500 files, 8 MiB); a larger source is not copied,
  because the diff renderer would refuse to show it anyway, and the re-gate
  degrades to the honest no-snapshot message.
  Write-once, keyed by exactly the checksum the lockfile records,
  never evicted — which is what lets both sides of a re-lock coexist so a diff
  has something to compare. Copies, never links, so a delivered or compared
  artifact cannot track later edits to the project file. Every read re-hashes
  the directory against its own name before trusting it.
- **The recognition index**, `~/.agentstack/recognition.json`. A map from
  content digest to the project keys that have approved it, written after a
  grant. It holds digests and project keys — **never content**. Revoking a
  project's trust removes it from the index.
- **Standing re-gate decisions**, stored on the project's entry in
  `~/.agentstack/trust.json`. What the human answered when content changed
  under a pin they had already approved: `keep-pinned` (with the pin to keep)
  or `blocked`. Discarded with the rest of consent when trust is revoked.

**What they are not:**

- **Not tamper-evident.** All three live under the user's own `~/.agentstack`
  at ordinary user permissions. A same-user process that can write there can
  write the trust store too; the real boundary is the OS user account, as
  stated at the top of this document. The card must never be read as attesting
  that the snapshot it diffs against is authentic — it attests that this is
  what *was recorded* when consent was given.
- **Not consulted by trust verification — but the snapshot store is a
  delivery source, not only a display one.** `check`/`check_digest` read the
  trust store and the pinned files, exactly as before, and no verdict of theirs
  moves if all three are deleted. The recognition index really is render-only:
  delete it and the card simply shows no recognition line. The other two are
  not. Standing decisions are read on five delivery paths — skill
  materialization under `use --write`, the MCP loadable-skill catalog,
  `skill_load`, instruction compilation, and the protected `run`, which refuses
  outright rather than partially: when an item is `blocked`, and when a
  `keep-pinned` item's project copy has drifted, since a protected run delivers
  the project copy and so cannot honour that decision. The protected run is the
  one of the five that reads no snapshot at all — the lock holds its line,
  because `keep-pinned` leaves the pin on the approved bytes. And a
  `keep-pinned` item is served **from** the snapshot store, by the digest the
  decision names, with the read re-proving the address first. So deleting
  `~/.agentstack/store/` drops every keep-pinned skill from materialization,
  excludes every keep-pinned instruction fragment from the compiled region,
  and makes `skill_load` refuse; the MCP catalog still lists the name — it is
  an inventory, and hiding it would conceal that the item exists at all — but
  the entry is marked `loadable: false`, carrying the reason and the action, so
  it is no longer an offer the loader would refuse. That is fail-closed and deliberately so — the approved
  bytes are what agents load, or nothing is. It is nevertheless a change in
  what is *delivered*, not only in what is *shown*.
  (`crates/cli/src/store.rs` `verified_snapshot`,
  `crates/cli/src/commands/use_profile.rs`,
  `crates/cli/src/render/instructions.rs`,
  `crates/cli/src/commands/locked.rs`, `crates/cli/src/mcp_server.rs`)
- **Not synced, and not portable.** None of the three is rendered into a
  project, committed, or shared. Recognition in particular never crosses
  machines — that is a consequence of where it lives, not a policy promise.
- **Not a backup.** The snapshot store is not a recovery mechanism and is not
  what `agentstack x restore` reads; it holds approved bytes for comparison, not
  project history.
- **Not a widening of consent.** Recognition shortens what the card *says*; it
  never shortens the gate. The per-project yes still happens in full, and a
  machine-level "always allow this content anywhere" is deliberately not built.

## Sharing, intake, and what a signature is worth

Phase 4 added three surfaces that all handle content from outside this machine.
Each is written down here because each is a place where a reader could
reasonably assume more protection than exists.

### Bundle signatures — `share` / `receive`

A `.astack` bundle carries an ed25519 signature over its own contents
(everything except the signature and the key that made it). What the signature
proves, exactly: **these bytes came from the holder of this key, unchanged
since they signed.**

What it does not prove, and must never be read as proving:

- **Nothing about whether the content is safe.** A publisher can sign malware
  perfectly well. Verification is an authenticity check, not a review.
- **Nothing about who the key belongs to.** There is no certificate authority
  and no web of trust here. A key is a stranger's until the user runs
  `agentstack publisher trust` and says whose it is — a purely local claim,
  based on whatever out-of-band check they did or did not perform.
- **It is not a second way to say yes.** A valid signature from a recognized
  publisher changes the card's wording — the question of *whose* key it is is
  settled — and changes nothing else. Recognition adds one dimmed line *above*
  the review body; the body itself is then printed by the same `summary_lines`
  loop either way, and that loop reads the bundle alone and never the
  publisher, so the body cannot vary by construction. What the witness
  (`crates/cli/tests/share_round_trip.rs`) pins byte for byte is the outcome
  rather than the wording: the receiving project's whole `.agentstack/` tree,
  path by path and byte by byte, must be identical after a recognized run and
  an unrecognized one. The two cards are never compared to each other. They are
  probed by substring instead — the recognized card must name the publisher and
  say the content is still the reader's to review, and neither card may claim
  the review got shorter.

Interactively, an unsigned bundle and an invalid signature are both stated on
the card and neither aborts: the full review stands in both cases. An invalid
signature is the loudest of the three, because it means the bytes changed after
signing.

Headlessly the rule inverts, and this is the one place a signature *does*
decide something. `receive --yes` refuses both — a headless accept leans
entirely on the signature, and neither an absent one nor a broken one holds.
What it demands is a **verified** signature, not a recognized publisher: a good
signature from a key nobody has ever trusted passes `--yes`, because whose key
it is remains the reader's question and `--yes` was never a way to answer it.
And with no `--yes` and no terminal there is no accept path at all — the
receive declines. (`crates/cli/src/commands/share.rs` `confirmed`,
`crates/cli/src/publisher.rs` `Provenance::verifies`)

### Quarantine — where intake waits

Fetched content is staged under `.agentstack/quarantine/` before the card is
shown, so what the card describes is what is on disk rather than what was in
memory. Inertness there is structural rather than enforced: the path is not an
intake directory, is named by no manifest entry, is on no search path, and is
reachable by no server. **There is no sandbox around it** — it is a directory
of files nothing is arranged to read. That is the whole mechanism, and it is
honest precisely because there is nothing to bypass.

Declining removes the directory. The property is the Phase 1 one: fetched then
declined leaves the project byte-identical with nothing to clean up later.

Path traversal is refused at one choke point (`quarantine::check_relative`),
allow-list shaped, called both by the caller and inside staging so the
guarantee belongs to the module rather than to call-site diligence.

### Attribution — `license` and `origin` in the lock

The lock schema carries `license` and `origin`, `Lock::upsert` preserves them
across a re-lock, and `agentstack share` reads them from the sender's lock onto
each bundle entry so a receiver's card can show provenance. The one wire not yet
connected is inbound capture: the production paths that build a `LockedSkill`
(`use`/`lock`/`add`) still write `license: None, origin: None`, so a locally
added skill records no attribution until that wire lands. What follows describes
the carry-forward and share behaviour, which are live; treat "recorded per
pinned skill" as the intended end state, not today's default for every add path.

Carried forward by `Lock::upsert` so an ordinary
re-lock cannot erase it. NOTICE/LICENSE text travels with the content rather
than being summarized into a tag.

The honest limit: **this records what a source declared, and verifies none of
it.** A registry claiming `Apache-2.0` gets `Apache-2.0` written down. AgentStack
does not check that the claim is true, that the publisher had the right to make
it, or that the NOTICE text is complete. It makes the obligation *visible and
durable*, which is strictly more than a promise and strictly less than a legal
review.

## Experimental `tools_execute`

This is a separate, machine-opt-in mode with a narrower runtime surface than a
whole harness sandbox. It is available only in builds with the `sandbox`
feature and has no host fallback.

| Property | Status | Mechanism and honest limit |
|---|---|---|
| Project identity | **enforced** | Current project digest must be in the trust store before files, Docker, relay, or upstream dispatch. Trust covers AgentStack manifest layers/lockfile, not every arbitrary repository file. |
| Enablement | **enforced** | Only `[experimental] tools_execute = true` in the machine manifest is consulted. The same table in a repo cannot enable it. |
| Tool authority | **enforced** | Immutable, exact namespaced grant; per-run authenticated relay checks membership and count; the existing gateway re-applies compiled machine ∩ project tool policy. Allowed tools can still have side effects. |
| Secrets | **enforced** | No resolved secret, gateway environment, or relay credential appears in guest env/result/events. Upstream processes still receive secrets that their declared server configuration authorizes. |
| Filesystem read | **enforced** | Guest sees only a private read-only `/app` mount containing source, JSON input, bootstrap, generated bindings, and relay token. The policy ruleset is mounted only into the sidecar. The guest does not receive workspace, AgentStack home, Docker socket, or host home mounts. Container/kernel escape is outside this claim. |
| Filesystem write | **enforced** | Read-only root and `/app`; only a 16 MiB `noexec,nosuid,nodev` `/tmp` tmpfs and one pre-created result-file bind are writable. Both are real kernel caps, by different mechanisms: the tmpfs size caps the mount, but a bind's bytes land in the host inode and no mount option bounds them, so the result file is bounded at the writer instead — a 4 MiB `RLIMIT_FSIZE` (`--ulimit fsize=`, soft == hard) covering every file in the container. A write past it fails with `SIGXFSZ`, and with capabilities dropped and `no-new-privileges` set the guest cannot raise its own hard limit. The 1 MiB `MAX_RESULT_BYTES` remains a separate **host-side read refusal** applied afterwards: an oversized result is rejected as invalid, never truncated. The write cap is deliberately four times the read refusal, so it can never clip a result the host would have accepted. The cap is per file, not aggregate; total host-disk exposure per execution is bounded because the bind is the only writable host path. |
| Direct egress | **enforced** | Internal Docker network has only the egress sidecar as peer. Its ordinary proxy requires an undisclosed separate token; the fixed raw relay reaches only the host execution relay. The host relay binds the narrowest interface the sidecar can still reach via `host.docker.internal`: the private, non-routable docker0 bridge gateway on a native Linux daemon, or the host loopback on Docker Desktop — never a LAN-facing interface. It stays reachable from Docker containers on the host (not from other LAN hosts). Where that narrow address is unknown or unbindable — a Linux host whose docker0 gateway could not be determined, a Linux host that cannot bind the gateway it chose (Docker-Desktop-on-Linux, whose gateway lives in the VM), or any platform that is not linux/macOS/Windows — the execution refuses to start instead of widening to `0.0.0.0`, and the refusal names `AGENTSTACK_RELAY_BIND`. That variable is the only route to a wildcard bind, and `AGENTSTACK_RELAY_BIND=0.0.0.0` is the operator accepting a LAN-reachable relay on purpose. Its random token, exact grant, bounded protocol, and execution-scoped lifetime are the control. No payload/content inspection occurs on allowed tool results. |
| Process isolation | **enforced** | Non-root uid/gid 65532, capabilities dropped, `no-new-privileges`, 128 MiB memory, one CPU, 32 PIDs, 4 MiB max file size. Docker's configured/default seccomp policy, Docker itself, and the host kernel remain trusted computing base; AgentStack does not yet ship a custom executor seccomp policy. |
| Limits | **enforced** | Machine-owned timeout, output, and call defaults are configurable only below compiled hard ceilings; requests may only narrow them. Aggregate stdout/stderr and separate result/source/input bytes, granted-tool count, and relay call count are bounded. A tool call already dispatched upstream cannot be revoked atomically. |
| Recording | **enforced** | Run log creation is required. Events store digests and metadata, never source/input/result/secret values; tool calls carry execution IDs and render beneath the execution in `agentstack report run`. Recording is evidence, not tamper-proof remote attestation. |
| Runtime supply chain | **partial** | Node image is pinned by repository digest. AgentStack does not yet publish an executor-specific SBOM, attestation, or independent scan, so the feature remains experimental. |

## See also

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — the layer model this matrix concretizes,
  especially Layer 3 (policy dimensions) and Layer 4 (runtime modes).
- [`../TODO.md`](../TODO.md) — the ordered current work and evidence gates.
- [`../STRATEGY.md`](../STRATEGY.md) — the product direction and outcome gates.
- [`../CHANGELOG.md`](../CHANGELOG.md) — release history.
