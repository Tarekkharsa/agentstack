# MCP profile lease example

This fixture proves the zero-file workflow against one real `agentstack mcp`
stdio process. It opens the `backend` profile, discovers and loads its local
skill, inspects the in-memory trail, freezes that observed set into a new
manifest profile, and closes the lease.

Run it from the repository root:

```bash
AGENTSTACK_BIN=target/debug/agentstack examples/mcp-profile-lease/run-demo.sh
```

Or, with an installed binary:

```bash
examples/mcp-profile-lease/run-demo.sh
```

The script works in a temporary copy and uses an isolated `AGENTSTACK_HOME`, so
it does not alter your project or personal AgentStack configuration. It also
asserts that the lifecycle creates none of the native-session artifacts:
`.mcp.json`, `.claude/skills/`, or `sessions.json`.

The important split is:

- `agentstack_lease_open` and `agentstack_lease_close` are MCP tool calls made
  by an agent through one live connection; they are not shell commands.
- `agentstack lock` is a human-side shell command. Run it after reviewing a
  profile created by `agentstack_lease_freeze` so `agentstack.lock` follows the
  manifest change.
- A lease is process-local. Another terminal cannot inspect it, and process
  exit drops it automatically.
