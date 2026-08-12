<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Undo AgentStack changes

Start with the simple timeline:

```bash
agentstack undo
```

It shows recent AgentStack-managed writes and the exact command for returning
to a point. Nothing changes until you add the shown `--write` form.

```console
recent changes (newest first)
  1  1m ago       init                                         7 files · Claude Code, Codex CLI

  pick a point: agentstack undo --to <n> --write
```

## Undo one recorded write

The script-friendly form is:

```bash
agentstack x restore
agentstack x restore --last --write
agentstack x restore <id> --write
```

The bare form lists every recorded point and every adapter backup:

```console
$ agentstack x restore
Recorded changes (newest first):

  18cb1a4c48  1m ago   global   gateway connect            2 files · Claude Code, Codex CLI
  18cb1a4c46  1m ago   project  init                       7 files · Claude Code, Codex CLI

Undo one with: agentstack x restore <id> --write (or --last for the newest)

Adapter config backups (content before our last write):
  Claude Code    global ~/.claude.json
  Codex CLI      global ~/.codex/config.toml

Restore one with: agentstack x restore <adapter> [--scope project] --write
```

Restore covers managed config-file writes such as settings, hooks,
instructions, and rendered server configuration. The revert is itself recorded,
so going one step too far remains recoverable.

## Remove legacy rendered configuration

When the gateway now serves MCP servers live but older native entries remain:

```bash
agentstack x unrender
agentstack x unrender --write
agentstack doctor
```

It removes only entries AgentStack owns. User entries remain.

## Remove this project's rendered setup

```bash
agentstack x uninstall --scope project --keep-home
agentstack x uninstall --scope project --keep-home --write
agentstack doctor
```

`--keep-home` preserves the central library, trust store, audit, and undo
history. The project manifest and lock remain, so the setup can be rebuilt.

AgentStack removes empty parent directories left by its managed skill delivery.
It preserves non-empty folders and any user-owned files. This cleanup uses
directory-empty semantics; it never recursively deletes an unknown provider
tree.

## Remove everything AgentStack manages on this machine

```bash
agentstack x uninstall
agentstack x uninstall --write
```

Without `--keep-home`, this also removes AgentStack's machine state after
removing managed provider entries. Preview carefully: the deleted trust store,
audit history, and local library state are not recoverable from the undo ledger
once that ledger is gone. A Git-backed central library remains recoverable from
its remote.

## What undo cannot restore

AgentStack can reverse its own recorded configuration writes. It cannot restore
a file an MCP server deleted, an arbitrary manual manifest edit, or another
program's change. Use Git or that program's recovery mechanism for those.

Removal is never blocked by stale trust. You do not need to approve a project
to make it inert.

Next: [Troubleshooting](../troubleshooting.md) ·
[See what happened](see-what-happened.md) · [Reference](../reference.md)
