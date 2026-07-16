# Multi-CLI web app — one setup, three CLIs

A small storefront web app (a Fastify service in `bundle/src`, with a couple of
docs) whose team ships across **three** agent CLIs at once — Claude Code, Codex,
and Cursor. Instead of hand-maintaining `.mcp.json`, `.codex/config.toml`,
`.cursor/mcp.json`, a `CLAUDE.md`, and an `AGENTS.md` — and watching them drift —
the team authors **one** `.agentstack/agentstack.toml` and lets `agentstack`
compile it into each CLI's native format. This example is the runnable proof
that the fan-out is faithful, secret-safe, and reproducible.

The manifest declares one HTTP MCP server (the team's internal API, whose bearer
token is a `${WEBAPP_API_TOKEN}` placeholder — never a literal), one house-rules
instruction fragment, and a profile that pulls one skill **by name** from the
central library. `assert.sh` seeds an isolated library with that skill, activates
the profile, and asserts the outcome on disk: the server lands in all three
native configs in their own shapes (Claude tags `type:"http"`, Codex nests an
`http_headers` sub-table, Cursor infers the transport), the house-rules marker
lands inside the managed region of both `CLAUDE.md` and `AGENTS.md`, the library
skill materializes as a symlink into `.claude/skills` and `.agents/skills`, and
the resolved token appears only in the native configs — never in the manifest or
lockfile.

This example also probes a deliberate rough edge: **Cursor**. Cursor's adapter
supports MCP but has **no instructions and no skills** support. Cursor is a
declared target, and the house-rules fragment targets `*` (all three CLIs), so
the honest question is what the user experiences. The disk answer is clean —
Cursor gets the server and nothing else — but the *communication* is not:
`agentstack` never warns that the instruction and skill can't reach Cursor, and
`agentstack explain house-rules` actively claims the fragment is "compiled into
each one's CLAUDE.md / AGENTS.md managed region" even though Cursor has neither.
`assert.sh` captures this verbatim and marks it as a documented defect (a `SKIP`,
not a `FAIL`) — the render is correct; the surfacing is not.

## How to run

```bash
bash assert.sh
```

It resolves the `agentstack` binary from `AGENTSTACK_BIN`, then `PATH`, then a
built `target/release/agentstack` in this repo. Everything runs inside an
isolated temp `AGENTSTACK_HOME` and `HOME` — your real config is untouched.

## What PASS proves

A green run (`N passed, 0 failed`) proves that one manifest fanned out into three
CLIs correctly and secret-safely:

- **Native shapes.** `.mcp.json` (Claude, `type:"http"`), `.codex/config.toml`
  (Codex, `[mcp_servers.storefront-api.http_headers]` sub-table), and
  `.cursor/mcp.json` (Cursor, no transport tag) each carry the server with the
  resolved token.
- **Portable instructions.** The `STOREFRONT-HOUSE-RULE-A7` marker sits inside
  the `<!-- agentstack:start -->` … `<!-- agentstack:end -->` region of both
  `CLAUDE.md` and `AGENTS.md`.
- **Library skill by name.** The profile references `api-conventions` by name;
  `assert.sh` seeds it into the central library, and it materializes as a symlink
  into `.claude/skills/api-conventions` and `.agents/skills/api-conventions` with
  the right `SKILL.md`.
- **Secret placement.** The resolved token is present in the three native configs
  (their formats store plaintext) and absent from both the manifest and the
  lockfile.

The single `SKIP` line is the finding, not a failure: Cursor silently receives no
instruction and no skill, and no `agentstack` surface (`apply`, `doctor`,
`explain`) warns about it.

## What it does not claim

This is a portability-and-surfacing example, not a runtime-enforcement one. It
renders configs; it does not spawn the server, broker any call, or firewall
anything. For the trust gate and tool firewall see `examples/malicious-repo-demo/`;
for kernel-enforced egress and filesystem confinement see
`agentstack run --sandbox`/`--lockdown`.
