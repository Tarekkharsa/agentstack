<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Add an MCP server

For a server you want across projects, store one reusable definition in the
central library and select it by name.

## 1. Write a safe definition

Use `${REF}` for secrets:

```toml
# github-server.toml
type = "http"
url = "https://api.githubcopilot.com/mcp/"

[headers]
Authorization = "Bearer ${GITHUB_TOKEN}"
```

## 2. Add it to the library

```bash
agentstack lib add-server github --file ./github-server.toml
agentstack lib add-server github --file ./github-server.toml --write
agentstack lib list
```

The preview validates the definition and shows the destination. The secret
value never enters the library.

## 3. Select it in a project

```toml
default_toolset = "backend"

[toolsets.backend]
servers = ["github"]
```

Then:

```bash
agentstack lock          # preview
agentstack lock --write
agentstack trust .
agentstack secret set GITHUB_TOKEN
agentstack status
```

The next trusted agent connection opens the default and can discover the
server's tools through `tools_search`. The server definition is not copied into
project MCP configs in the live lane.

Because nothing is written for a server served live, `x why` is the place that
answers "where did this come from, and who gets it":

```console
$ agentstack x why github

  github  (MCP server)

    from      the central library · init:local
    pinned    sha256:b720188932b4…
    approved  yes · you said yes 0s ago
    live      Claude Code · Codex CLI
    written   Claude Code — in its own config, which AgentStack does not manage
    scope     runs `/usr/bin/env` · reads GITHUB_TOKEN
    used      never activated from here yet

    full detail: agentstack explain github
```

## Other starting points

| What you have | Command |
| --- | --- |
| A catalog name | `agentstack search <name>` then `agentstack add from <id>` |
| A native server already configured | `agentstack adopt` to preview importing it |
| A server only this project needs | `agentstack add server ...` |
| An existing manifest definition to reuse | `agentstack lib add-server <name> --from-manifest` |

After any selected server change, use the same lock-then-trust loop. Run
`agentstack apply --write` only when status reports a rendered compatibility
lane that requires native files.

Next: [Central library](../library.md) · [Secrets](../concepts.md#secrets) ·
[Server reference](../reference.md#adopt-and-add)
