# `tools_execute` threat model

Status: experimental implementation, 2026-07-13. The authoritative claim
matrix is [`../ENFORCEMENT.md`](../ENFORCEMENT.md#experimental-tools_execute).

## Assets and trust boundaries

Protected assets are host files and credentials, machine policy, project trust
identity, gateway authority, upstream MCP credentials, Docker/host networking,
and the integrity of execution evidence. Inputs treated as hostile include the
TypeScript source, JSON input, repository manifest, upstream descriptions and
results, and every relay byte. Docker, the host kernel, AgentStack CLI/gateway,
and the pinned Node image are trusted computing base.

The boundary is:

```text
hostile request
  → CLI trust check + immutable plan
  → private execution files
  → non-root lockdown container
  → token-authenticated exact-grant relay
  → existing Gateway (machine ∩ project policy)
  → upstream MCP server
```

The executor crate does not read policy, manifests, trust, environment, or
secrets. The CLI is the only composition point. The gateway is the only tool
authority. The egress crate owns asynchronous relay parsing and transport; the
runtime owns container isolation; the recorder owns evidence.

## Threats and mitigations

| Threat | Mitigation | Residual risk / test evidence |
|---|---|---|
| Hostile source reads host/workspace/credentials or policy internals | No workspace/home/socket mounts; private `/app` only; policy ruleset is in a separate sidecar-only control directory; read-only root; Node permission model as defense in depth | Docker/kernel/image escape remains TCB. Real-Docker test denies `/etc/passwd` and `/app/ruleset.json`; mount construction has no ambient paths. |
| Hostile source writes persistence | Read-only root and `/app`; 16 MiB `noexec,nosuid,nodev` tmpfs; one pre-created result-file bind; recursive permission restoration followed by ephemeral forced teardown | Kernel/runtime vulnerabilities remain. |
| Direct TCP/DNS/public internet | Internal-only network with sidecar as sole peer; ordinary proxy needs an undisclosed token; only fixed raw relay is usable | Docker topology is TCB. Test denies public TCP and verifies unauthenticated proxy CONNECT receives 407. |
| Ungranted or misspelled tool | Generated SDK exposes grant only; relay performs exact `BTreeSet` membership before callback; gateway applies policy again | Allowed tools may be powerful. Unit/integration tests prove ungranted calls do not dispatch. |
| Confused deputy / repository enables execution | Machine manifest is the only feature flag; project trust digest checked before files, relay, Docker, or calls | Machine owner intentionally opting in accepts the experimental surface. |
| Relay spoofing/reuse | Random per-execution token, constant-time comparison, listener lifetime bound to execution, strict schema and 1 MiB frame cap | Token is readable by guest because the guest must authenticate; grant makes possession non-widening. Listener binds host interfaces for Docker reachability, so token is mandatory. |
| Relay memory/connection/call/reactor DoS | Reader capped before newline, response cap, eight concurrent connections, global atomic call ceiling, and blocking gateway callbacks dispatched outside the Tokio reactor | An already-dispatched upstream call can outlive guest cancellation; relay shutdown does not wait for it. |
| CPU/memory/process/output DoS | 1 CPU, 128 MiB, 32 PIDs, 60 s hard wall ceiling, 256 KiB hard output ceiling, forced container removal | Host Docker daemon availability remains operational dependency. Real-Docker tests cover infinite loop and excessive output. |
| Malicious upstream result becomes protocol/control data | Relay serializes result as JSON; SDK parses one framed response; result remains data; fixed public error classes; final result uses a separate bounded file rather than stdout framing | Guest code can intentionally act on returned data, as designed. |
| Secret leakage through logs/errors | No secrets in guest env; events use digests/metadata; gateway reduces upstream failures to fixed classes; MCP errors are stable | An allowed upstream tool can return sensitive data to the guest/result. Policy must govern that tool. |
| Audit forgery or omission | Run-log creation required; execution/tool/limit/finish events are host-written and calls carry execution ID | Local logs are not remote/tamper-proof attestation. Early setup failures currently produce start evidence plus the stable caller error; no source/value is logged. |
| Container/relay survives timeout or disconnect | Bounded wait followed by forced container removal; lockdown and relay are RAII-owned; relay runtime uses background shutdown | External side effects already accepted by an upstream cannot be rolled back atomically. |

## Security review checklist before stabilization

- Independently trace request source to Docker, relay, gateway, recorder, and
  result sinks.
- Fuzz relay framing and request/result normalization beyond property tests.
- Soak repeated timeout, output, upstream-hang, and concurrent-client cases;
  check Docker containers/networks and host threads for leaks.
- Publish executor runtime provenance, supported architectures, SBOM, scan,
  digest update policy, and attestation.
- Revisit mutating-tool approval using explicit metadata; never infer mutation
  from a tool name.
