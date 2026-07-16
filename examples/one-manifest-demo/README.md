# One manifest, every CLI

A runnable, recordable proof of AgentStack's **portability wedge**: you author
your agent setup once, and `agentstack apply` renders it into each CLI's own
native config format — no hand-editing three files, no drift between them.

```bash
./run-demo.sh              # fast, asserting; exits nonzero on any mismatch
DEMO_PAUSE=2.5 ./run-demo.sh   # paced, for an asciinema recording
```

Requires `agentstack` on `PATH` (or `AGENTSTACK_BIN=/path/to/agentstack`, or a
built `target/release/agentstack` in this repo) and `python3`. It runs entirely
in an isolated temp `HOME` — nothing touches your real config.

## The bundle

`bundle/.agentstack/agentstack.toml` is the single source of truth. It:

- targets three CLIs — **Claude Code, Codex, Cursor**;
- declares **one** stdio MCP server (`github`);
- carries **one** portable instruction fragment (`team-guardrails.md`);
- references its token only as the placeholder **`${GITHUB_TOKEN}`** — no secret
  value lives in the manifest.

## What the demo proves

1. **The portable artifact is secret-free.** The committed manifest holds
   `${GITHUB_TOKEN}`, never a value. The demo greps it to prove this before
   rendering anything.

2. **One manifest fans out into three native formats.** `agentstack apply`
   compiles the same server into each CLI's own shape, at each CLI's own path:

   | CLI         | File                 | Native quirk                                   |
   |-------------|----------------------|------------------------------------------------|
   | Claude Code | `.mcp.json`          | tags transport as `"type": "stdio"`            |
   | Codex       | `.codex/config.toml` | `[mcp_servers.<name>]` table + `env` sub-table |
   | Cursor      | `.cursor/mcp.json`   | infers the transport, omits the `type` tag     |

   The instruction fragment compiles into `CLAUDE.md` (Claude Code) and
   `AGENTS.md` (Codex), inside a managed marker block. Cursor has no instruction
   file agentstack manages — agentstack renders Cursor's MCP config, but not its
   instructions.

3. **Preview before write.** `agentstack apply` with no `--write` is a read-only
   plan — it shows the per-CLI diff and masks the secret as `${GITHUB_TOKEN}`,
   then writes nothing. `--write` applies.

4. **Secrets resolve per-machine, honestly.** The token is resolved at apply
   time from the environment (the resolution chain is env → varlock → OS
   keychain → `.env`, fail-closed if unset). The demo sets a fake value in the
   env, then asserts the honest outcome: the resolved token now sits in each
   **rendered native config** — those formats store plaintext — while the
   **manifest and lockfile still hold only the placeholder**. AgentStack keeps
   the secret out of the portable artifact; it does not, and does not claim to,
   keep it out of the native files whose own formats require a literal.

The script ends with `PASS`/`FAIL` assertions on every one of these outcomes and
exits nonzero if any fails, so it doubles as a CI-grade regression check.

## What it does not claim

This demo is about **portability and secret placement**, not runtime
enforcement. It does not spawn the server, broker any call, or firewall
anything — see `examples/malicious-repo-demo/` for the trust gate and tool
firewall, and `agentstack run --sandbox`/`--lockdown` for kernel-enforced
egress and filesystem confinement.
