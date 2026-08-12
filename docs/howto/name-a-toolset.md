<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Name a toolset

A toolset is the small group of MCP servers and skills one kind of task needs.
It selects capabilities; it does not grant extra permission.

## Create one

```bash
agentstack toolset create backend --server github --server postgres --skill api-review
agentstack toolset list
```

At a terminal, `create` shows the change and asks before writing. It updates the
manifest and lock but does not render native MCP or skill files.

The result is simple TOML:

```toml
[toolsets.backend]
servers = ["github", "postgres"]
skills = ["api-review"]
```

## Make it automatic

```bash
agentstack toolset default backend
agentstack toolset default backend --write
agentstack trust .
```

A trusted new gateway connection opens the default automatically. There is no
daily `use` command in the normal zero-files workflow.

If a project declares exactly one toolset, AgentStack can use it as the
effective default. Declaring `default_toolset` is still clearer for people and
becomes necessary when the project has several toolsets.

## Several tasks

```toml
default_toolset = "backend"

[toolsets.backend]
servers = ["github", "postgres"]
skills = ["api-review", "sql-review"]

[toolsets.incident]
servers = ["grafana", "logs"]
skills = ["oncall-runbook"]
```

New connections open `backend`. A protected run can choose another fence:

```bash
agentstack run claude-code --toolset incident
```

To move every new request onto another toolset, change the project default with
`agentstack toolset default <name> --write`; to pin one launch instead, use the
launcher flag above. A modern connection re-derives the trusted default on its
next request, so it picks the change up without reconnecting. Only a legacy
connection keeps its opening selection — connection-scoped toolset leases are
legacy-only, and the binary refuses one for a modern connection.

## After editing a toolset

```bash
agentstack lock          # preview
agentstack lock --write
agentstack trust .
agentstack status
```

Use `agentstack use <name> --write` only for a file-only CLI or an intentional
rendered compatibility lane. It is not how live zero-files switching works.

Next: [Central library](../library.md) · [Trust](trust-a-repo.md) ·
[Concepts](../concepts.md)
