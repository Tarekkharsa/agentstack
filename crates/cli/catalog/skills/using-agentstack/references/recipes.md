# AgentStack recipes

## Add a reusable capability

1. Preview adding it to the personal library with `agentstack lib add ...` or
   `agentstack lib add-server ...`.
2. Add its name to the relevant `[toolsets.<name>]` list in the project
   manifest. Do not copy its files into the project.
3. Run `agentstack lock` to preview. Leave `agentstack lock --write` and the
   review to the human.
4. Run `agentstack status`. A live-routed capability appears on the next trusted
   agent connection without a native config write.

## Bring an existing machine under management

`agentstack init` detects native MCP entries and supported settings, previews
the import, and leaves the source files in place. It does not import native
skills automatically. App-bundle servers and built-in capabilities remain
owned by their application.

To make existing user-owned skills reusable, preview:

```bash
agentstack adopt --to-library
```

Use `--write` only after the user approves the plan. Do not adopt app-owned
Computer Use, connectors, plugins, or bundled MCP plumbing unless the user
explicitly asks to override the exclusion.

## Change a toolset or its default

Use the project contract command:

```bash
agentstack toolset default rust
agentstack toolset default rust --write
```

The first command previews; the second updates this manifest field and
refreshes the lock:

```toml
default_toolset = "rust"

[toolsets.rust]
servers = ["upstash/context7", "gha-search"]
skills = ["rust-testing"]
```

Then refresh the lock and ask the human to review trust. Existing connections
keep their frozen selection; reconnect to receive the changed default.

## Fix a missing secret

Keep `${NAME}` in the server definition. Tell the user to run:

```bash
agentstack secret set NAME
agentstack status
```

Never request, print, copy, or commit the value.

## Review drift

1. Run `agentstack status`; use `agentstack doctor` when it requests the deeper
   check, then follow its exact first fix.
2. If the manifest is authoritative, preview `agentstack apply`; use
   `--write` only for a rendered lane.
3. If a deliberate native hand-edit should be kept, preview
   `agentstack adopt --write` instead.
4. Refresh the lock when pinned content changed.
5. Leave `agentstack trust .` to the human after the final bytes are stable.

## Bootstrap another machine

First bootstrap:

```bash
agentstack up --library <git-url>
agentstack up --library <git-url> --write
agentstack status
```

Later refresh:

```bash
agentstack up
agentstack up --write
agentstack status
```

Expect the new machine to ask only for its missing secret names and its local
trust review. Do not copy trust stores, vault values, audit logs, or generated
CLI configuration from another machine.

## Remove legacy configuration

Use AgentStack's ownership-aware cleanup; do not delete provider folders by
hand:

```bash
agentstack x unrender --write
agentstack x uninstall --scope project --keep-home --write
agentstack doctor
```

Only AgentStack-managed entries are removed. Empty managed skill parent folders
are cleaned automatically; user-owned files and non-empty folders remain.
