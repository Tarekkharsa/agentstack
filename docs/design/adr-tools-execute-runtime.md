# ADR: runtime and ownership for `tools_execute`

- Date: 2026-07-13
- Status: accepted for experimental use

## Decision

Use the official Node 22 slim container pinned by repository digest, with
TypeScript type stripping inside the same container. Support Docker-capable
Linux/macOS only for v1 and fail closed everywhere else. Do not add host
execution fallback.

The runtime identity is
`node:22-slim@sha256:53ada149d435c38b14476cb57e4a7da73c15595aba79bd6971b547ceb6d018bf`,
protocol version 1. Source imports only the generated
`agentstack:runtime` virtual binding; AgentStack rewrites that fixed import to
the mounted offline module. There is no npm install, remote module resolution,
native addon, worker, or subprocess permission.

Ownership is fixed as follows:

- `executor`: policy-agnostic request/plan/grant/limit/error types and backend
  traits; depends only on `core`, backend-neutral `runtime`, and `recorder`.
- `runtime`: hardened container specification, bounded wait, and teardown.
- `egress`: asynchronous strict framed relay transport.
- `cli`: trust, machine opt-in, `Arc<Gateway>` adapter, runtime composition,
  generated SDK, and MCP exposure.
- `gateway`: sole tool-policy decision and upstream dispatch point.
- `recorder`: execution evidence and parent/child attribution.

An `executor → policy` or `executor → cli` dependency is prohibited unless the
architecture is deliberately reopened.

## Why Node for the experimental implementation

The existing Docker test environment already supports a multi-architecture
official Node image, Node 22 executes the required TypeScript subset with
`--experimental-strip-types`, and its permission model removes filesystem,
worker, and subprocess surfaces as defense in depth. The real security boundary
is still the Docker mount/network/resource topology and exact-grant gateway
relay—not the language runtime permission model.

Measured on the maintainer's Docker 25/macOS environment, the complete focused
test (lockdown sidecar, executor, one real stdio gateway call, filesystem and
network probes) completes in roughly 1.2 seconds warm; the timeout and output
cases bring the three-execution test to roughly 3 seconds. These are test-suite
observations, not portable performance guarantees.

QuickJS would reduce runtime surface but adds TypeScript compilation and SDK
compatibility work; Deno adds another image/update surface without improving
the topology boundary. Neither alternative was benchmarked comprehensively,
so this ADR does not claim Node won a universal performance comparison. The
choice is intentionally reversible while the schema is experimental.

## Consequences

- Release builds already compile the sandbox feature; local source builds need
  `cargo build --features sandbox`.
- Docker and the AgentStack egress sidecar image are operational prerequisites.
- Docker host-bridge reachability requires the short-lived authenticated relay
  to bind host interfaces rather than loopback. The per-execution token, exact
  grant, frame/connection caps, and call ceiling are mandatory compensating
  controls.
- AgentStack relies on Docker's configured/default seccomp behavior and does
  not yet ship a custom executor seccomp profile; Docker and the host kernel
  remain trusted computing base.
- The executor image is digest-pinned, but AgentStack does not yet publish an
  executor-specific SBOM/attestation/scan. The feature cannot leave
  experimental status until that supply-chain evidence and a focused security
  review exist.
- Successful source is not persisted as a package. Promotion, scheduling,
  background jobs, arbitrary dependencies, and general network APIs remain out
  of scope.
