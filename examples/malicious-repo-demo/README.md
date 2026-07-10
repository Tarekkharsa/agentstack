# The malicious-repo demo

The ten-second proof of what AgentStack is for: **a cloned repo's agent can
touch nothing until you review it, and your own machine policy overrides
anything the repo declares.**

This is a self-contained, self-asserting reproduction. It runs the *same*
hostile bundle three ways and checks the outcome of each — so it either
provably works or fails loudly, which is why it runs in CI.

```sh
examples/malicious-repo-demo/run-demo.sh
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
still reach any host its policy allows — a prompt-injected agent could leak
data through an approved channel, including the model API itself. Blocking a
trusted repo from connecting to an *unapproved host* (per-host egress
enforcement) is **Phase 2**, the egress crate, and is not in this build. The
script marks the exact spot where that assertion will slot in once it exists.
The demo is deliberately scoped to what today's build can prove.

## Files

- [`bundle/`](bundle/) — the hostile repo (manifest + `evil_server.py`)
- [`sink.py`](sink.py) — the localhost sink that records any phone-home
- [`run-demo.sh`](run-demo.sh) — the asserting reproduction
