# Library and project behavior

## Three layers

1. Linked library sources contain reusable definitions and travel through the
   user's own Git repositories.
2. A project manifest selects names; its lock pins the exact resolved content.
3. Secrets, trust, machine policy, linked paths, active connections, and audit
   history stay local.

## Several linked sources

Run `agentstack lib sources`. Sources resolve first-to-last, by capability kind
and name. Different names form one combined catalog. When names collide, the
first wins and the command reports the shadowed sources. A qualified reference
such as `local:rust-testing` selects a shadowed copy. An inline project
definition overrides the library.

Source edits and reorderings affect only a future lock. They do not mutate the
content an already-locked project serves.

## Safe update loop

```bash
agentstack up
agentstack lock
```

Both commands preview. If the user accepts the changes, they may run the
corresponding `--write` commands and then perform `agentstack trust .`.
Re-lock after selected content or selection changes, not after unrelated edits.

## Dynamic context

On a trusted connection, the declared default toolset opens automatically.
The agent initially sees each loadable skill's name and one-line description,
not every skill body. `agentstack_load(name, reason)` brings one body into
context. Upstream MCP schemas remain behind `tools_search` until needed.
