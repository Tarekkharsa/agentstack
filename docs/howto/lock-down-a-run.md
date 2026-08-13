<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Lock down a run

For anyone launching an agent on work that must not leak. Prerequisite for the
sandbox tiers: a running Docker daemon and a build with sandbox support (release
binaries have it; a bare `cargo build` needs `--features sandbox`).

```bash
# Preview first — walks every gate, launches nothing, needs no Docker
agentstack run claude-code --sandbox --lockdown --plan

# Then climb only as far as you need:
agentstack run claude-code                       # protected host run, no Docker (the default)
agentstack run claude-code --sandbox             # container + proxied egress
agentstack run claude-code --sandbox --lockdown  # container, no route out
```

Each step confines more, and each prints its [posture](../concepts.md) label —
what each label actually guarantees is the
[enforcement matrix](../ENFORCEMENT.md#the-matrix):

- A plain `run` is **already** the Protected tier. No Docker. It
  enforces content trust, strict [lockfile](../concepts.md) verification, and
  policy admission **before** launch, and freezes the tool surface for the run.
  It is not isolation — the agent still runs as you, on the host. Posture:
  `HOST / PROTECTED`. `--locked` names the same run explicitly and still works;
  `--unprotected` opts out of the gate entirely (posture `HOST / ADVISORY`, and
  the banner names each check that was skipped).
- `run --sandbox` launches the CLI inside a Docker container with the project
  mounted as its workspace and HTTPS routed through a host-side egress proxy.
  The container's bridge network still has a direct route a proxy-ignoring
  process could use. Posture: `SANDBOX / PROXIED · DIRECT ROUTE OPEN`.
- `run --sandbox --lockdown` puts the container on an internal-only network
  whose sole peer is the egress sidecar — no host route, no internet. Posture:
  `LOCKDOWN / ENFORCED · NO DIRECT ROUTE`.

**Content trust is the gate you meet first.** "Enforces content trust before
launch" means a protected run of an untrusted or drifted project is refused, not
downgraded. `--plan` tells you so without launching anything, and the message
carries the fix in the order it has to happen:

```text
error: a live `run claude-code --locked` would be REFUSED — 1 blocker:
  [trust] configuration changed since it was trusted — re-review and re-trust.
  If you changed pinned inputs, run `agentstack lock --write` first — new pins
  re-gate trust.
```

Lock, then `agentstack trust .`, then run — see
[trust a cloned repo](trust-a-repo.md). `--unprotected` is the only way past it,
and it drops the whole pre-launch gate, not just this check.

**`--unprotected` is interactive only, and deliberately so.** It is the one
posture that is not available to a script:

```text
agentstack run claude-code --unprotected --plan          # exit 1
agentstack run claude-code --unprotected --prompt "…"    # exit 1
```

Both refusals launch nothing and both are intended, for different reasons:

- `--plan` previews the pre-launch **gate**. `--unprotected` switches that gate
  off, so there are no gate decisions left to preview. Use the protected plan
  instead — `agentstack run claude-code --plan` launches nothing either, and it
  walks the very checks `--unprotected` would skip, which makes it the honest
  preview of what opting out gives up.
- `--prompt` is headless delivery, and headless delivery is defined only by the
  protected contract: the argv is committed into a frozen grant and the output
  is recorded as bounded evidence. `--unprotected`, `--sandbox`, and
  `--lockdown` have no such contract. An unattended run is the one nobody is
  watching, so it is the one that keeps its gate — CI gets
  `agentstack run <cli> --prompt "…"`, the protected form, and nothing looser.

So an `--unprotected` run always has a person at the terminal reading its
`HOST / ADVISORY` banner. Dropping protection is not something a script can do
on your behalf.

Point `AGENTSTACK_SANDBOX_IMAGE` at an image that carries your agent CLI. The
lockdown egress sidecar is pulled from GHCR automatically, pinned per release
(override with `AGENTSTACK_EGRESS_IMAGE`).

After a run, `agentstack more report run <id>` replays its posture label and every
egress and tool-call decision — see [see what your agents did](see-what-happened.md).

**Limits.** The posture labels name each mode's ceiling honestly: the protected
default is pre-launch gating plus a frozen surface, not a kernel fence — the
harness still runs as you, on the host — and only
`--lockdown` is topologically confined. What each mode actually enforces per
dimension, with every strength caveat, is the
[enforcement matrix](../ENFORCEMENT.md#the-matrix).

- [Concepts](../concepts.md) — sandbox vs lockdown vs the protected run, posture
- [Reference: execution posture](../reference.md#execution-posture)
- [Enforcement: the matrix](../ENFORCEMENT.md#the-matrix)
