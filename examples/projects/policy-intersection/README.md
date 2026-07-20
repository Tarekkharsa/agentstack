# Policy intersection — the machine layer is a floor no repo can loosen

A runnable, CI-grade proof of AgentStack's **two-layer policy model**, exercised
end-to-end through the real MCP gateway. The effective policy an agent runs
under is the **intersection** of the repo's policy and the machine's policy, and
this example proves the machine layer is a floor: a repo can narrow it, but it
can never widen it — not even for a tool the repo explicitly allowlisted for
itself.

```bash
bash assert.sh              # fast, asserting; exits nonzero on any mismatch
DEMO_PAUSE=2.5 bash assert.sh   # paced, for an asciinema recording
```

Requires `agentstack` on `PATH` (or `AGENTSTACK_BIN=/path/to/agentstack`, or a
built `target/release/agentstack` in this repo) and `python3`. It runs entirely
inside an isolated sandbox — a temp `AGENTSTACK_HOME` and `HOME` — so nothing
touches your real config, machine manifest, trust store, or audit log.

## The repo

`bundle/` is a cloned repo that ships a tiny stdio MCP server, `opsbox`
(`bundle/server.py`), exposing four tools: `get_status` and `list_items`
(read-only), `delete_everything` (destructive), and `admin_reset` (privileged).

The interesting part is what its manifest (`bundle/.agentstack/agentstack.toml`)
does with policy. It **allowlists `delete_everything` for itself**:

```toml
[policy.tools]
opsbox = ["get_*", "list_*", "delete_everything"]
```

A repo firewalling itself is worth nothing — a hostile or careless repo will
always allow whatever it wants. So `assert.sh` writes the **machine layer**
(`$AGENTSTACK_HOME/agentstack.toml`) that the user controls and no repo can
touch:

```toml
[policy.tools]
"*" = ["!delete_*", "!*_admin", "!admin_*"]
```

The `"*"` key is rename-proof: it denies those tool-name shapes on **every**
server, so a repo can't dodge the floor by renaming its server. The manifest
also declares `[policy.egress]` and `[policy.secrets]` entries for `opsbox` so
you can see all three policy dimensions compile through `explain`.

## What the demo proves

1. **Untrusted means inert.** Before `agentstack trust`, the gateway
   (`agentstack mcp --auto-project`) serves only its own control plane.
   `tools_search` reports the project is not trusted, a direct
   `opsbox__get_status` call is rejected as an unknown tool, and the audit log
   stays empty — the server is never spawned or contacted.

2. **The floor filters discovery.** Once trusted, `tools_search` surfaces only
   `get_status` and `list_items`. `delete_everything` is **invisible** even
   though the repo allowlisted it — the machine floor removes it from discovery
   entirely, so the agent never learns the tool exists. `admin_reset` is
   invisible too.

3. **The floor firewalls execution.** `opsbox__get_status` is allowed and
   returns `ok`. `opsbox__delete_everything` and `opsbox__admin_reset` are
   refused, and the refusal text **names the machine layer** and the exact rule
   (`denied by [policy.tools] rule "!delete_*" (machine policy —
   ~/.agentstack/agentstack.toml)`) — so it's unambiguous which layer said no.

4. **Every call is audited.** `$AGENTSTACK_HOME/audit/calls.jsonl` records the
   allowed call with `"outcome":"ok"` and each denied call with
   `"outcome":"denied"` plus the rule and layer that denied it.

5. **`explain` shows both layers.** `agentstack explain opsbox` prints the
   project tool policy, the machine tool policy (with "this project cannot
   loosen it"), and the egress and secret dimensions.

6. **`doctor` labels the machine-policy summary.** `agentstack doctor` reports
   the machine-policy summary as **restrictive** — a rename-proof `"*"` rule
   constrains every server.

The script ends with `PASS`/`FAIL` assertions on every one of these outcomes and
exits nonzero if any fails, so it doubles as a regression check. A PASS proves
that the effective policy is genuinely the intersection of the two layers, that
the intersection is enforced at both discovery and call time through the real
gateway, and that a repo cannot escalate past the machine floor it declared for
itself.

## What it does not claim

This demo is about the **tool-policy intersection and the gateway firewall**,
not kernel-enforced egress. The `[policy.egress]` entry is declared and shown by
`explain`, but runtime per-host egress filtering is the separate
`agentstack run --sandbox --lockdown` primitive — see
`examples/malicious-repo-demo/` for the trust gate and tool firewall against a
genuinely hostile server, and the `one-manifest-demo/` for the portability
story.
