<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# See what your agents did

For anyone who wants to inspect what their agent CLIs ran, called, and cost —
and to vet a capability before relying on it. Prerequisite: none for `explain`;
the run and call reports need activity AgentStack actually
brokered (through the [gateway](../concepts.md) or `agentstack run`) — a plain
host run that talks to servers directly leaves nothing to report.

| To see… | Run |
| --- | --- |
| Every tracked run, one line each | `agentstack report runs` |
| One run in full — lifecycle, [egress](../concepts.md#egress), tool calls, secret refs, [posture](../concepts.md) | `agentstack report run <id>` |
| What each capability costs you in context | `agentstack report usage` |
| Every brokered tool call — ok, denied, errored | `agentstack report calls` |
| What the `tools` block actually costs per turn, on the wire | `agentstack report wire` |
| Concrete fixes from the signals already collected | `agentstack optimize` |
| One server or skill, **before** you trust or rely on it | `agentstack explain <name>` |

```bash
# What ran, and what one run did
agentstack report runs                # table of tracked runs (--json to script)
agentstack report run <id>            # one run's flight recorder + posture label
agentstack kill <id>                  # stop a tracked run that's gone wrong

# What your capabilities cost and call
agentstack report usage               # per-capability context cost + activation counts
agentstack report usage --live        # measure each server's tools/list on the wire
agentstack report calls --since 7     # brokered tool calls, last 7 days (--json to script)
agentstack report wire                # per-turn token weight of the tools block

# Turn evidence into next actions
agentstack optimize                   # evidence-backed recommendations (--write applies the safe class only)

# Before you trust or rely on a capability
agentstack explain <name>             # provenance, effective policy, and context cost, for one capability
```

**After a run.** `report runs` lists tracked runs; `report run <id>` reads that
run's [flight recorder](../concepts.md) — its full lifecycle, egress, tool-call,
and secret-ref record — plus the run's [posture](../concepts.md) label. A run
that's still going and shouldn't be: `agentstack kill <id>` stops it and
reverts any profile it owned.
`report calls` summarizes the global [call audit log](../concepts.md) across all
runs (argument *digests* only, never values). The two context lenses differ:
`report usage --live` estimates a server's `tools/list` footprint, `report wire`
measures what the `tools` block actually costs on the wire.

**Optimize.** `agentstack optimize` turns the same signals into
recommendations, each carrying its evidence and the exact command or TOML;
`--write` applies only the provably-inert safe class. The same machine-readable
reports feed external tools and integrations.

**Before you trust.** `agentstack explain <name>` is the vet-first command: it
shows one server or skill's origin and provenance, whether it has drifted from
its pin, its [effective policy](../reference.md#mcp-firewall-policytools) (the
tool, egress, and secret-access rules that will actually apply), and its
per-session context cost. Run it before `agentstack trust .` (see
[trust a cloned repo](trust-a-repo.md)) or before adding a capability to a
profile.

**Limits.** `report calls`, `report usage`, and `optimize` only see
gateway-brokered calls — a server rendered into a native config is called by the
harness directly, so it never appears in the audit log and is never auto-removed
on "no calls" evidence alone. These records are best-effort local diagnostics,
not tamper-evident forensics. What each mode actually enforces (and records) is
the [enforcement matrix](../ENFORCEMENT.md#the-matrix).

- [Concepts](../concepts.md) — flight recorder, call audit log, posture
- [Reference: live runs and `report`](../reference.md#live-runs-agentstack-run)
- [Reference: optimize](../reference.md#optimize-agentstack-optimize)
