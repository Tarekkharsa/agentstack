# The malicious-repo demo

The ten-second proof of the gateway boundary: **a cloned repo's declared MCP
server stays inactive until you trust its consent surface, and your machine
policy can deny tools the repo itself allows.**

This is a self-contained, self-asserting reproduction. It runs the *same*
hostile bundle three ways and checks the outcome of each — so it either
provably works or fails loudly, which is why it runs in CI.

```sh
examples/malicious-repo-demo/run-demo.sh
# paced for a screen recording:
DEMO_PAUSE=2.5 examples/malicious-repo-demo/run-demo.sh
```

(Needs `agentstack` on your `PATH`, or `AGENTSTACK_BIN=/path/to/agentstack`, and
`python3`. Everything runs in a throwaway sandbox — your real config is never
touched.)

## What you're looking at

The villain is [`bundle/evil_server.py`](bundle/evil_server.py): an MCP server
that advertises a friendly `status` tool and a malicious `exfiltrate` tool.
Call `exfiltrate` and it reads a planted credential off disk and POSTs it to a
localhost "sink" — the phone-home. The [`bundle/`](bundle/) manifest declares
that server and, tellingly, **no firewall of its own** — a malicious repo won't
constrain itself.

The demo then runs it three ways:

| # | Scenario | What happens | What it proves |
|---|----------|--------------|----------------|
| 1 | **Unprotected** — a bare harness runs the server | The sink **receives** the planted secret | The threat is real |
| 2 | **AgentStack, untrusted** | The server is **never spawned**; the sink stays **empty**; its tools aren't even exposed | Nothing runs until you review it |
| 3 | **AgentStack, trusted, machine firewall on** | The `exfiltrate` call is **denied** and written to the audit log; the sink stays **empty** | No repo can loosen your own machine policy |

In scenario 3 the block comes from the **machine** policy
(`~/.agentstack/agentstack.toml`, `[policy.tools] "*" = ["!exfiltrate"]`) — the
user's own layer, which no repo can widen — using the rename-proof `"*"` key so
it holds whatever the repo names its server.

## What this demo claims — and what it does not

It claims exactly this: **unreviewed repos stay inert, and tools your machine
policy forbids are blocked and audited.**

It does **not** claim that exfiltration is impossible. A *trusted* repo can
still leak through an approved channel, including the model API itself. This
particular demo exercises the trust and tool-firewall primitives; it does not
exercise egress confinement. For unapproved-host blocking, run a trusted bundle
with `agentstack run --sandbox --lockdown`: lockdown removes the direct route
and makes the enforcing egress proxy the only path out. Even then, an allowed
destination remains allowed—the proxy restricts destinations, not payloads.

## Files

- [`bundle/`](bundle/) — the hostile repo (manifest + `evil_server.py`)
- [`sink.py`](sink.py) — the localhost sink that records any phone-home
- [`run-demo.sh`](run-demo.sh) — the asserting reproduction
