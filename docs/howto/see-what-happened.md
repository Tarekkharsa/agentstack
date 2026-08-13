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
| Every tracked run, one line each | `agentstack more report runs` |
| One run in full — lifecycle, [egress](../concepts.md#egress), tool calls, fence refusals, secret refs, [posture](../concepts.md) | `agentstack more report run <id>` |
| What each capability costs you in context | `agentstack more report usage` |
| Every brokered tool call — ok, denied, errored | `agentstack more report calls` |
| What the `tools` block actually costs per turn, on the wire | `agentstack more report wire` |
| Concrete fixes from the signals already collected | `agentstack more optimize` |
| One server or skill, **before** you trust or rely on it | `agentstack more explain <name>` |

```bash
# What ran, and what one run did
agentstack more report runs                # table of tracked runs (--json to script)
agentstack more report run <id>            # one run's flight recorder + posture label
agentstack more kill <id>                  # stop a tracked run that's gone wrong

# What your capabilities cost and call
agentstack more report usage               # per-capability context cost + activation counts
agentstack more report usage --live        # measure each server's tools/list on the wire
agentstack more report calls --since 7     # brokered tool calls, last 7 days (--json to script)
agentstack more report wire                # per-turn token weight of the tools block

# Turn evidence into next actions
agentstack more optimize                   # evidence-backed recommendations (--write applies the safe class only)

# Before you trust or rely on a capability
agentstack more explain <name>             # provenance, effective policy, and context cost, for one capability
```

**Start here:** `agentstack more report runs`. If it is empty, nothing has been
brokered yet — runs are tracked when launched with `agentstack run`, and calls
are brokered through the [gateway](trust-a-repo.md).

**A call that was refused is not a call the run made.** `report run <id>` lists
tool calls the [fence](../concepts.md#lease-session-or-protected-run-fence)
turned away in their own **Fence refusals** section, naming the server, the tool,
the toolset it was asked through, and the reason — deliberately kept out of the
tool-call count above it, so the number a reviewer reads is what the run
actually did.

**Before you trust.** `agentstack more explain <name>` is the vet-first command: it
shows one server or skill's origin and provenance, whether it has drifted from
its pin, its [effective policy](../reference.md#mcp-firewall-policytools) (the
tool, egress, and secret-access rules that will actually apply), and its
per-session context cost. It reads and reports only, so it is not itself gated —
it works in the untrusted repo you are trying to make a decision about. That is
the point: run it before `agentstack trust .` (see
[trust a cloned repo](trust-a-repo.md)) or before adding a capability to a
toolset.

**Limits.** `report calls`, `report usage`, and `optimize` only see
gateway-brokered calls — a server rendered into a native config is called by the
harness directly, so it never appears in the audit log and is never auto-removed
on "no calls" evidence alone. These records are best-effort local diagnostics,
not tamper-evident forensics. What each mode actually enforces (and records) is
the [enforcement matrix](../ENFORCEMENT.md#the-matrix).

- [Concepts](../concepts.md) — flight recorder, call audit log, posture
- [Reference: live runs and `report`](../reference.md#live-runs-agentstack-run)
- [Reference: optimize](../reference.md#optimize-agentstack-more-optimize)
