# Lock down a run

For anyone launching an agent on work that must not leak. Prerequisite for the
sandbox tiers: a running Docker daemon and a build with sandbox support (release
binaries have it; a bare `cargo build` needs `--features sandbox`).

```bash
# Preview first — walks every gate, launches nothing, needs no Docker
agentstack run claude-code --sandbox --lockdown --plan

# Then climb only as far as you need:
agentstack run claude-code --locked              # protected host run, no Docker
agentstack run claude-code --sandbox             # container + proxied egress
agentstack run claude-code --sandbox --lockdown  # container, no route out
```

Each step confines more, and each prints its [posture](../concepts.md) label —
the honest measure of how strongly the policy is *enforced*, not just declared:

- `run --locked` promotes a plain host run to the Protected tier. No Docker. It
  enforces content trust, strict [lockfile](../concepts.md) verification, and
  policy admission **before** launch, and freezes the tool surface for the run.
  It is not isolation — the agent still runs as you, on the host. Posture:
  `HOST / PROTECTED`.
- `run --sandbox` launches the CLI inside a Docker container with the project
  mounted as its workspace and HTTPS routed through a host-side egress proxy.
  The container's bridge network still has a direct route a proxy-ignoring
  process could use. Posture: `SANDBOX / PROXIED · DIRECT ROUTE OPEN`.
- `run --sandbox --lockdown` puts the container on an internal-only network
  whose sole peer is the egress sidecar — no host route, no internet. Posture:
  `LOCKDOWN / ENFORCED · NO DIRECT ROUTE`.

Point `AGENTSTACK_SANDBOX_IMAGE` at an image that carries your agent CLI. The
lockdown egress sidecar is pulled from GHCR automatically, pinned per release
(override with `AGENTSTACK_EGRESS_IMAGE`).

**Limits.** Only lockdown is topologically confined, and even there the honest
claim is *unapproved egress is blocked* — never that exfiltration is impossible,
since a host you allowed can still receive data. `--locked` is pre-launch gating
plus a frozen surface, not a kernel fence. What each mode enforces, per
dimension, is the [enforcement matrix](../ENFORCEMENT.md#the-matrix).

- [Concepts](../concepts.md) — sandbox vs lockdown vs `--locked`, posture
- [Reference: execution posture](../reference.md#execution-posture)
- [Enforcement: the matrix](../ENFORCEMENT.md#the-matrix)
