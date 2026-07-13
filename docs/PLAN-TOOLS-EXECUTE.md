# Plan: governed `tools_execute`

Status: implemented behind experimental machine opt-in; stabilization gates open  
Owner: maintainer  
Last updated: 2026-07-13

## Outcome

Add one compact MCP tool that accepts a small TypeScript program, executes it
in an isolated runtime, and lets that program call only explicitly granted,
policy-approved MCP tools through the existing gateway.

The intended loop is:

```text
tools_search → generate TypeScript → tools_execute
                                      │
                                      ├─ trust gate
                                      ├─ effective machine ∩ project policy
                                      ├─ isolated runtime
                                      ├─ gateway-brokered MCP calls
                                      └─ run log + call audit
```

The first release is deliberately ephemeral. It does not persist packages,
schedule jobs, provide memory, or introduce general-purpose assistant state.

## Why this belongs in agentstack

Agentstack already provides nearly every security and portability primitive
needed by a hosted code-mode executor:

- `tools_search` provides compact, on-demand discovery.
- `Gateway::try_call` is the policy and audit enforcement point for upstream
  MCP calls.
- `CompiledRuleset` is the single machine/project policy artifact.
- `Gateway::from_plan` provides hard trust gating and avoids policy drift.
- `agentstack-runtime` already provides container lifecycle and lockdown
  networking.
- `RunLog` and the gateway call log provide run-level evidence.
- generated code-mode bindings establish the desired TypeScript API.

The new feature should connect these pieces. It must not create a second MCP
client, policy evaluator, secret resolver, audit path, or trust model.

## Product boundary

### In the first release

- One TypeScript source string per MCP call.
- An explicit list of allowed namespaced tools.
- A bounded JSON-compatible input value.
- A JSON-compatible result plus bounded stdout/stderr diagnostics.
- No ambient secrets, filesystem, or internet.
- Every upstream call passes through `Gateway::try_call`.
- Run metadata and terminal outcome are recorded.
- Linux-container execution through the existing Docker runtime.

### Explicitly excluded

- Persisted or shareable packages.
- Cron, jobs, workflow orchestration, retries, or background execution.
- OAuth onboarding and integration creation.
- Memory, values, storage, or databases.
- Arbitrary npm installation.
- Native host execution as an automatic fallback.
- Direct network access from the executor.
- Access to the user's workspace unless a future policy explicitly grants a
  narrow mount.

## Implementation gate and sequencing

This section records the original sequencing gate. On 2026-07-13 the maintainer
explicitly authorized implementation on `feat/gateway-unification`. The feature
therefore landed behind both a compile-time sandbox feature and a machine-owned
experimental flag; it is not declared stable and does not imply the remaining
roadmap prerequisites are complete. The original gate was:

1. `feat/gateway-unification` is reviewed, merged, and its canonical gateway
   path is stable on the target branch.
2. ROADMAP Session B is complete: sandbox filesystem enforcement has kernel-
   level deny-glob mask mounts and the intended fine-grained read/write scopes.
3. Remaining v0.9 readiness blockers are closed.
4. The `agentstack run --locked <bundle-or-repo>` hero path is implemented and
   verified, proving the complete trust → compiled policy → gateway → lockdown
   → recorder composition with existing workloads.
5. Usage evidence shows that `tools_search` and `tools_bindings` solve real
   workflows and that harness-owned execution is the remaining reliability,
   portability, or governance gap.

The implementation covers Milestones 0–4 and CLI reporting from Milestone 5.
Independent review, soak/fuzz depth, executor-specific SBOM/attestation, and
reusable packages remain open; the enforcement docs label those gaps.

## Security invariants

These are release gates, not aspirations:

1. **Untrusted means no execution.** A missing, changed, or untrusted bundle
   returns an error before starting a runtime, resolving a secret, or spawning
   an upstream server.
2. **No second authority path.** Generated code can invoke MCP tools only via
   the same `Gateway` instance used to construct the advertised surface.
3. **Explicit grant plus policy intersection.** A tool is callable only when it
   is both named in the request grant and allowed by the compiled ruleset.
4. **No ambient credentials.** The executor receives no inherited environment,
   keychain access, Docker socket, agentstack home, host home, or secret value.
5. **No ambient filesystem.** The initial executor has a temporary writable
   working directory and read-only runtime assets only. The project workspace
   is not mounted.
6. **No direct network.** The executor has no general egress route. Its only
   communication channel is the per-execution, token-authenticated gateway
   relay.
7. **Bounded resources.** Wall time, memory, CPU, process count, output bytes,
   result bytes, and MCP call count have hard limits.
8. **Cancellation kills the tree.** Timeout, client cancellation, or gateway
   teardown terminates the executor and every descendant.
9. **No secret-bearing evidence.** Logs contain source and argument digests,
   tool identities, decisions, timings, and outcomes—never raw source by
   default, resolved secret values, authorization tokens, or complete tool
   results.
10. **One immutable execution context.** Trust decision, authority/ruleset
    digest, profile fence, tool grant, runtime limits, and run identity are
    captured once before the process starts and do not re-read ambient state
    during execution. The executor records the authority digest but does not
    interpret the ruleset.

### Invariant traceability

| # | Enforcement mechanism | Proof required before experimental release |
| ---: | --- | --- |
| 1 | CLI plan builder requires `TrustState::Trusted`; gateway construction uses the hard-gated `Gateway::from_plan` path | Changed, missing, and untrusted bundle integration tests show no container, secret resolution, or upstream spawn |
| 2 | One CLI-owned `ToolAuthority` adapter delegates calls only to the session's `Arc<Gateway>` | Test authority records every dispatch; code search and dependency audit show no second MCP client in executor/runtime |
| 3 | Exact immutable `ToolGrant` checked by relay; `Gateway::try_call` repeats compiled-policy enforcement | Unknown, ungranted, project-denied, and machine-denied calls all fail before upstream side effects |
| 4 | Empty allowlisted environment, no home/keychain/agentstack mounts, secrets resolved only by gateway | Container probes and event/log scans prove credentials and values never enter executor state |
| 5 | Read-only root, per-run tmpfs/result mount, no project mount | Container read/write probes cover workspace, home, agentstack home, root, tmpfs, and result paths |
| 6 | Internal Docker network whose only peer is the fixed-destination egress relay | TCP, UDP, DNS, HTTPS, proxy bypass, and direct-upstream tests all fail while granted relay calls succeed |
| 7 | Request clamping plus container CPU/memory/pid limits and protocol byte/call/concurrency counters | Boundary tests at limit − 1, limit, and limit + 1 for every resource dimension |
| 8 | Runtime timeout/cancellation owns container, relay, network, and descendant lifecycle | Infinite loop, sleeping/forked child, client disconnect, and forced relay failure leave no process/network artifacts |
| 9 | Digest-only recorder schema; bounded redacted diagnostics; secret-aware gateway logging | Golden event tests plus adversarial source/input/output/upstream strings containing fake secrets and protocol text |
| 10 | Immutable `ExecutePlan`; no manifest, policy, session, or environment reads after construction | Mutation/TOCTOU tests change disk and ambient state after plan creation without changing the running authority |

## MCP contract

Add `tools_execute` to the compact control-plane surface in
`crates/cli/src/mcp_server.rs`.

Proposed input schema:

```json
{
  "type": "object",
  "required": ["code", "allowTools"],
  "properties": {
    "code": {
      "type": "string",
      "description": "TypeScript source; must export or return a JSON-compatible result"
    },
    "allowTools": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Exact namespaced tools, e.g. github__get_issue"
    },
    "input": {
      "description": "JSON-compatible value exposed to the program as input"
    },
    "limits": {
      "type": "object",
      "properties": {
        "timeoutMs": { "type": "integer" },
        "maxCalls": { "type": "integer" },
        "maxOutputBytes": { "type": "integer" }
      }
    }
  }
}
```

Client-supplied limits may only reduce machine defaults. They can never raise
them. Unknown keys should be rejected to keep the authority request explicit.

Proposed successful result:

```json
{
  "content": [{
    "type": "text",
    "text": "{\"runId\":\"x-…\",\"result\":{…},\"calls\":3,\"ms\":842}"
  }],
  "isError": false
}
```

Errors use stable categories such as `untrusted`, `invalid-input`,
`tool-not-granted`, `policy-denied`, `runtime-unavailable`, `timeout`,
`resource-limit`, `execution-error`, and `invalid-result`. Internal paths,
tokens, command lines, and raw backend errors must not be exposed to the MCP
client.

## Program contract

The generated program should use a tiny preinstalled SDK:

```ts
import { tools, input } from "agentstack:runtime";

const issue = await tools.github.get_issue({ owner: "acme", repo: "app", number: input.number });
const comments = await tools.github.list_comments({ owner: "acme", repo: "app", number: input.number });

export default {
  title: issue.title,
  openComments: comments.filter((comment) => !comment.resolved).length
};
```

The SDK is generated from the granted subset only. Referencing an ungranted
binding should fail before or during module evaluation, and the gateway repeats
the grant check server-side so SDK generation is never treated as enforcement.

For v1, support a conservative TypeScript subset compiled ahead of execution.
There is no package resolution except the virtual `agentstack:runtime` module
and an intentionally small standard library. Dynamic imports, CommonJS
`require`, native modules, eval-like constructs, and subprocess APIs should be
unavailable at the runtime boundary rather than detected with source scanning.

## Architecture

Introduce a domain boundary rather than implementing execution inside
`mcp_server.rs`:

```text
MCP handler
  └─ ExecuteService::run(ExecuteRequest, ExecutionAuthority)
       ├─ validate request and limits
       ├─ verify exact grant against Gateway::describe/namespaced_tools
       ├─ construct one execution plan
       ├─ ask egress-owned relay to open a per-execution channel
       ├─ run Executor backend
       ├─ collect bounded result
       └─ append execution events

Executor backend
  ├─ compile/bundle TypeScript
  ├─ construct minimal SandboxSpec
  ├─ execute via agentstack-runtime
  └─ enforce teardown and resource limits

Runtime SDK relay
  └─ exact grant check → Gateway::try_call → existing policy/audit/upstream
```

### Proposed modules

Create a new workspace crate, `crates/executor`, to keep untrusted-code
execution out of the CLI and to make its invariants unit-testable.

```text
crates/executor/
  src/lib.rs          public ExecuteService-facing types and errors
  src/request.rs      validated request, grant, and effective limits
  src/runner.rs       backend-neutral Executor trait and orchestration
  src/sdk.rs          generated virtual TypeScript SDK
  src/container.rs    Docker/runtime adapter (feature-gated)
  src/events.rs       executor event construction
```

The asynchronous relay and its framed wire protocol belong to `crates/egress`,
not `crates/executor`:

```text
crates/egress/
  src/execution_relay.rs    async listener, authentication, framing, limits
  src/execution_protocol.rs wire messages and strict decoder
```

`executor` receives a narrow synchronous lifecycle interface for opening,
describing, and closing the channel. It does not acquire tokio/hyper or own a
socket runtime. This preserves the repository rule that the async enforcement
stack stays confined to `egress` and prevents a transport choice from leaking
through the execution domain.

Proposed dependency direction:

```text
executor → core, runtime, recorder
egress   → core, policy, recorder
cli      → everything (existing composition rule, now including executor)
```

The existing `cli → everything` edge lets CLI composition use both `executor`
and `egress`; no `executor → egress` edge is required if CLI supplies the relay
lifecycle implementation. If implementation proves that a direct edge is
unavoidable, that is a new architecture decision requiring its own review—not
an incidental `Cargo.toml` change.

These edges must be added to the exact edge table in `CLAUDE.md` and the crate
diagram in `docs/ARCHITECTURE.md` during Milestone 0, before either crate imports
the other. The executor must not depend on `policy`: it consumes an immutable,
already-authorized `ToolGrant`, while `Gateway::try_call` remains the sole
per-call policy enforcement point.

The executor crate must not depend on the CLI crate. Because `Gateway` currently
lives in `crates/cli`, define a narrow callback trait in `executor`:

```rust
pub trait ToolAuthority: Send + Sync {
    fn describe(&self, name: &str) -> Option<ToolDescriptor>;
    fn call(&self, grant: &ToolGrant, name: &str, args: Value) -> ToolResult;
}
```

Implement that trait for a CLI-owned adapter around `Arc<Gateway>`. Do not move
the entire gateway during the first milestone. A later cleanup may extract
gateway functionality into its own crate after behavior is locked by tests.

### Execution plan

Capture every decision in one immutable value:

```rust
pub struct ExecutePlan {
    pub execution_id: String,
    pub parent_run_id: Option<String>,
    pub project_digest: String,
    pub authority_digest: String,
    pub source_digest: String,
    pub input_digest: String,
    pub grant: ToolGrant,
    pub limits: EffectiveLimits,
    pub runtime: RuntimeIdentity,
    pub sdk: GeneratedSdk,
}
```

`ExecutePlan` contains no secret values. The plan builder performs trust and
grant validation; the runner consumes the finished plan without consulting the
manifest, environment, session file, or machine policy again.

## Runtime design

### Container image

Publish a small, pinned executor image containing:

- a pinned JavaScript runtime;
- a pinned TypeScript compiler/bundler;
- the agentstack executor bootstrap;
- no package manager metadata or shell-oriented development toolchain unless
  strictly required by the bootstrap.

Reference it by immutable digest in release metadata. The binary and image must
have an explicit compatibility version. Startup fails closed on mismatch.

Do not use `npx`, download dependencies at runtime, or trust a mutable `latest`
tag.

### Filesystem

Create a new temporary directory per execution containing only:

```text
/run/agentstack/source.ts       read-only after creation
/run/agentstack/sdk.ts          read-only
/run/agentstack/input.json      read-only
/run/agentstack/result/         writable, size-bounded
```

Mount no workspace and no agentstack home. Use a read-only root filesystem,
tmpfs for temporary runtime files, a non-root uid, dropped Linux capabilities,
`no-new-privileges`, process-count limits, and a restrictive seccomp profile.

### Network and relay

Reuse lockdown topology rather than plain proxy environment variables. The
executor container joins an internal per-execution network with no internet
route. Its only peer is a fixed-destination relay that accepts the runtime
protocol and calls the in-process `ToolAuthority`.

The relay implementation lives in `agentstack-egress`, which already owns the
async network boundary. `agentstack-executor` owns only backend-neutral relay
requirements and the synchronous lifecycle trait supplied by CLI composition.
This ownership decision is fixed before Milestone 1; the first implementer must
not choose it implicitly by adding an async dependency.

Use a random, per-execution bearer token delivered through a mounted 0400 file,
not a command-line argument. The relay binds only for the execution lifetime.
The token authenticates the process; it does not grant tools. The immutable
`ToolGrant` remains the authorization source.

The protocol should be length-prefixed or newline-delimited JSON with strict
message and nesting limits. It needs only:

- `call { id, tool, arguments }`
- `result { id, value }`
- `error { id, category, message }`
- `complete { result }`
- `diagnostic { stream, chunk }`

Reject duplicate IDs, excessive concurrency, oversized frames, non-object tool
arguments, messages after completion, and more calls than the plan permits.

### Limits

Start with conservative defaults. Timeout, call, and diagnostic-output
defaults are configurable in the machine-level manifest only; compiled hard
ceilings remain non-configurable and requests may only narrow the effective
values:

| Limit | Initial default | Hard ceiling |
| --- | ---: | ---: |
| Source | 64 KiB | 256 KiB |
| Input JSON | 64 KiB | 1 MiB |
| Wall time | 15 s | 60 s |
| Memory | 128 MiB | 512 MiB |
| CPU | 1 core | 2 cores |
| Processes | 32 | 64 |
| MCP calls | 20 | 100 |
| Concurrent MCP calls | 4 | 8 |
| stdout + stderr | 64 KiB | 256 KiB |
| Final result | 256 KiB | 1 MiB |

Treat these numbers as starting hypotheses. Validate them with representative
benchmarks before freezing the public defaults.

## Trust and policy flow

1. Resolve the project exactly as `mcp --auto-project` does.
2. Require `TrustState::Trusted`; changed or missing trust returns an empty
   execution authority.
3. Build or reuse the same gateway and `CompiledRuleset` for the MCP session.
4. Normalize and deduplicate `allowTools`; reject empty, wildcard, or unknown
   grants.
5. Verify every requested tool appears in the policy-filtered
   `Gateway::namespaced_tools()` surface.
6. Pin the profile fence and tool descriptors into `ExecutePlan`.
7. During every relay call, check exact grant membership again.
8. Call `Gateway::try_call`, which repeats tool policy enforcement and records
   the upstream call.

An initially allowed call may still fail at runtime because the upstream is
unavailable or its tool schema changed. That is an execution error, not a reason
to weaken validation or bypass the gateway.

## Recording and reporting

Extend `agentstack-recorder::RunEvent` with additive, version-tolerant events:

```rust
ExecutionStarted {
    ts, execution_id, parent_run_id, source_digest, input_digest,
    runtime_digest, granted_tools, limits
}
ExecutionFinished {
    ts, execution_id, outcome, duration_ms, calls, result_digest,
    stdout_bytes, stderr_bytes
}
ExecutionLimitHit { ts, execution_id, limit, observed }
```

Do not store raw source or result by default. Add an explicit future debug mode
only after defining retention, redaction, and user consent behavior.

Gateway call records already capture server, tool, arguments digest, outcome,
and latency. Attribute those records to the execution ID as well as the parent
run so `agentstack report` can render a tree:

```text
run r-123
└─ execution x-456 (842 ms, 3 calls, ok)
   ├─ github.get_issue        120 ms  ok
   ├─ github.list_comments    208 ms  ok
   └─ slack.post_message      blocked policy.tools.machine
```

## Delivery milestones

### Milestone 0 — threat model and decision record

Deliverables:

- Add a focused threat model covering hostile source, malicious upstream tool
  results, relay abuse, container escape assumptions, denial of service,
  confused deputy risks, audit leakage, and cancellation.
- Write an ADR selecting the runtime, image distribution model, and isolation
  boundary.
- Amend `CLAUDE.md` and `docs/ARCHITECTURE.md` with the exact new crate edges;
  reject `executor → policy` unless the architecture is deliberately reopened.
- Fix relay ownership in the ADR: async transport and protocol implementation
  in `egress`, backend-neutral execution orchestration in `executor`, and CLI
  composition between them.
- Define `ExecuteRequest`, `ExecutePlan`, stable error categories, defaults,
  and hard ceilings.
- Decide supported platforms. Recommended v1: Docker-capable Linux/macOS hosts;
  fail clearly elsewhere.

Exit gate: all implementation prerequisites are satisfied; every security
invariant maps to one enforcement mechanism and at least one planned test; the
maintainer has explicitly approved the architecture amendment and authorized
Milestone 1.

### Milestone 1 — pure execution domain

Deliverables:

- Create `crates/executor` with request parsing, normalization, grants, limits,
  plan construction, errors, and `ToolAuthority`/`Executor` traits.
- Implement a fake executor and fake authority for unit tests.
- Add recorder event variants and report parsing without changing existing
  report output.

Exit gate: exhaustive unit tests cover malformed input, limit clamping, exact
grants, duplicate grants, unknown tools, result validation, timeout mapping,
and safe error rendering.

### Milestone 2 — runtime SDK and relay

Deliverables:

- Generate SDK bindings only for the granted tools.
- Implement the framed runtime protocol with strict byte/count limits inside
  `egress`.
- Implement the CLI adapter from `Arc<Gateway>` to `ToolAuthority`.
- Add a loopback-only test transport first; it is test scaffolding, not a
  production execution mode.

Exit gate: integration tests prove an allowed tool works, an ungranted tool is
blocked before gateway dispatch, a policy-denied tool remains denied, malformed
frames fail closed, and concurrency/call limits hold.

### Milestone 3 — isolated container executor

Deliverables:

- Build and pin the executor image.
- Add minimal container specification support to `agentstack-runtime`: read-only
  root, tmpfs, uid, capabilities, process/memory/CPU limits, and result mount.
- Implement internal-network relay topology using the lockdown machinery.
- Implement timeout and process-tree teardown.

Exit gate: end-to-end tests with Docker prove no workspace, home, keychain,
Docker socket, arbitrary host, DNS, or public internet is reachable; only
granted gateway calls succeed.

### Milestone 4 — MCP exposure behind an experimental flag

Deliverables:

- Add the `tools_execute` definition and handler to `mcp_server.rs`.
- Add a machine-level opt-in, recommended name:
  `experimental.tools_execute = true`.
- Keep `tools_bindings` for compatibility and for harness-owned execution.
- Update `tools_search` detail responses to offer both paths when enabled.
- Document installation of the executor image and failure modes.

Exit gate: the compact `tools/list` remains bounded; disabled installations do
not advertise `tools_execute`; enabled but runtime-unavailable installations
return a clear, non-leaky error.

### Milestone 5 — reporting, hardening, and beta

Deliverables:

- Render execution trees in `agentstack report` and the dashboard.
- Add property/fuzz tests for protocol parsing and request normalization.
- Add soak tests for repeated executions, cancellation, upstream hangs, and
  concurrent clients.
- Commission a focused security review of source-to-sink paths.
- Publish the image provenance, checksums/attestation, SBOM, and compatibility
  policy.

Exit gate: no open critical/high findings, resource cleanup is demonstrated,
and all claims are reflected honestly in `ENFORCEMENT.md`.

### Milestone 6 — reviewed reusable packages

Begin only after ephemeral execution has production evidence.

Deliverables:

- Add a promotion command that turns successful source into a content-addressed
  library item.
- Store source, declared grants, input schema, output schema, runtime identity,
  and tests—never embedded credentials.
- Include package digest in the lockfile and trust review.
- Require re-review on any source, grant, dependency, runtime, or policy change.
- Execute packages through the identical `ExecuteService` path.

Non-goal: a package is not a background job. Scheduling remains a separate
future decision.

## Test matrix

### Unit tests

- Source/input/result size boundaries at `limit - 1`, `limit`, and `limit + 1`.
- Timeout and numeric overflow handling.
- Grant normalization and exact-match semantics.
- Machine limit always wins over larger request value.
- Stable public error category for every internal error.
- Source, input, and result digests are deterministic.
- Recorder serialization remains backward compatible.

### Gateway integration tests

- Allowed HTTP and stdio tools execute through `Gateway::try_call`.
- Tool denied by machine policy is never dispatched.
- Tool denied by project policy is never dispatched.
- Unknown and ungranted tools are indistinguishable to generated code except
  for the safe public error category.
- Profile-fenced tools cannot be recovered by spelling their names manually.
- Upstream output containing protocol-shaped or prompt-injection text remains
  inert data.
- Secrets resolve only in the gateway/upstream process and never enter executor
  mounts, env, diagnostics, or events.

### Container end-to-end tests

- Read `/workspace`, `$HOME`, agentstack home, keychain paths, and Docker socket:
  all fail.
- Write outside the result/tmpfs area: fails.
- Direct TCP, UDP, DNS, public HTTPS, and proxy-environment bypass: all fail.
- Fork bomb/process flood: bounded and torn down.
- Infinite loop and sleeping child: timeout kills the whole tree.
- Excess stdout/stderr: truncated and terminated according to policy.
- Oversized/malformed result: rejected without loading it unboundedly.
- Relay token reuse after completion: fails.
- Client disconnect: runtime and relay are torn down.

### Regression tests

- Existing compact and transparent MCP surfaces are unchanged when disabled.
- Existing `tools_search`, `tools_bindings`, generated clients, sandbox runs,
  and gateway HTTP tests continue to pass.
- Builds without the sandbox/Docker feature still compile; they do not
  advertise execution unless another secure backend is explicitly added.

## Documentation changes

Update these files as implementation lands:

- `docs/ARCHITECTURE.md`: insert the execution service between gateway and
  sandbox; state that generated code is untrusted.
- `docs/ENFORCEMENT.md`: add one `tools_execute` matrix covering trust, tools,
  secrets, filesystem, egress, process isolation, limits, and recording.
- `docs/reference.md`: replace the future-tense `tools_execute` paragraph with
  its exact contract and limits.
- `docs/HISTORY.md`: record each shipped milestone and any claim adjustment.
- `README.md` and `docs/index.html`: mention the feature only after the
  enforcement matrix is verified.
- `docs/mcp-capability-layer.html`: link to the released reference once stable.

## Rollout and compatibility

1. Land domain types and tests with no user-visible behavior.
2. Land runtime/image support behind compile-time and machine-level flags.
3. Advertise the MCP tool only when explicitly enabled and the compatible image
   is available.
4. Run an experimental release cycle with telemetry limited to counts, timings,
   outcomes, and limits—never source, input, output, or secrets.
5. Freeze the request schema only after beta feedback; add optional fields
   compatibly thereafter.
6. Keep `tools_bindings` as a lower-level escape hatch for users who prefer
   execution in their harness sandbox.

## Implementation order by repository area

| Order | Area | Primary work |
| ---: | --- | --- |
| 1 | `crates/executor` | Domain API, grants, limits, protocol, fake backend |
| 2 | `crates/recorder` | Execution events and parent/child attribution |
| 3 | `crates/runtime` | Minimal hardened spec fields and container backend |
| 4 | `crates/cli/src/gateway.rs` | Narrow `ToolAuthority` adapter and execution attribution |
| 5 | `crates/cli/src/mcp_server.rs` | Schema, feature advertisement, request routing |
| 6 | `crates/cli/src/commands/report.rs` | Execution tree reporting |
| 7 | `crates/cli/src/dashboard` | Optional beta visibility after CLI reporting is stable |
| 8 | Documentation | Architecture, enforcement matrix, reference, release claims |

## Implementation decisions

1. **JavaScript runtime:** Node 22 slim, pinned by repository digest. Its
   permission model is defense in depth; Docker topology and gateway grants are
   the security boundary. See the [ADR](design/adr-tools-execute-runtime.md).
2. **Compilation boundary:** Node strips TypeScript types inside the same
   sandbox; hostile source never enters a broader host compiler.
3. **Image distribution:** official digest-pinned default. No user-supplied
   executor-image override is provided; arbitrary
   runtime substitution would reopen the reviewed boundary.
4. **Execution availability:** Docker/lockdown only; no host fallback.
5. **Approval semantics:** trust + exact grant + compiled policy. Explicit
   mutating-tool metadata/approval is deferred; mutation is never guessed from
   names, so allowed tools may still have side effects.
6. **Result retention:** digest-only events; the result is returned to the MCP
   caller but not persisted as an artifact.

### Runtime decision scorecard

Node was selected for the reversible experimental implementation. Deno and
QuickJS were not comprehensively benchmarked, so the table records evidence
rather than invented comparative scores.

| Criterion | Weight | Deno | QuickJS | Node |
| --- | ---: | ---: | ---: | ---: |
| Offline deterministic module control | 5 | not measured | not measured | fixed mounted modules; no package install |
| Container cold start | 4 | not measured | not measured | focused warm E2E ≈1.2 s on Docker 25/macOS |
| Peak memory under representative code | 4 | not measured | not measured | hard container ceiling 128 MiB; profiling still open |
| Cancellation and process-tree behavior | 5 | not measured | not measured | timeout + forced container removal tested |
| No native/add-on/package escape surface | 5 | not measured | favorable | no npm/addons/workers/subprocess permission; container remains boundary |
| TypeScript implementation complexity | 3 | favorable | compiler required | native type stripping in selected Node 22 |
| Multi-architecture image availability | 3 | favorable | varies | official multi-architecture image |
| Image size and patch cadence | 3 | varies | favorable | official slim image; digest updates remain maintainer work |
| Maintainer familiarity and debugging | 2 | moderate | lower | selected; ordinary Node diagnostics |

Regardless of score, the chosen runtime is untrusted application machinery.
The container, network topology, grant enforcement, and gateway remain the
security boundaries.

## Definition of done

`tools_execute` is ready to leave experimental status only when:

- all ten security invariants have implemented mechanisms and passing tests;
- generated code cannot reach the workspace, host network, credentials, or
  ungranted MCP tools in the supported runtime;
- the same compiled ruleset governs gateway calls and the runtime topology;
- timeout and cancellation demonstrably remove every process and network;
- execution and upstream calls appear in one attributable report;
- the executor image is pinned, reproducible, scanned, and attested;
- `ENFORCEMENT.md` distinguishes every enforced, advisory, and unsupported
  property without overclaiming;
- the feature fails closed when trust, Docker, image compatibility, policy,
  relay authentication, recording required for the mode, or teardown cannot be
  established.
