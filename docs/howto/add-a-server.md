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
value never enters the library. `lib list` then shows it under `Servers`, and
closes with a **What is dead in here** section listing every library entry with
no recorded usage — a new one starts there, as `no data`.

> **If you ran `agentstack init` first, pick a name it did not already import.**
> `init` puts each imported server in your first linked library source, so
> `lib add-server github` afterwards can land a *second* `github` in a
> *different* source. That is not an error and nothing is overwritten — sources
> resolve in order and the first match wins, so the new one silently shadows the
> imported one. `agentstack lib sources` prints the order and a `Shadowed names`
> section naming what is hidden, and `local:github`-style qualified names reach
> the shadowed copy. See
> [Link your central library](../start.md#2-link-your-central-library). Adding
> the same name twice in the *same* source is refused outright: `'github' is
> already in the central library — pass --replace to overwrite`.

## 3. Select it in a project

This goes in the project's manifest, `.agentstack/agentstack.toml`. Add the
`[toolsets.backend]` table, and **replace** the existing `default_toolset` value
— TOML allows the key only once, and `init` already wrote
`default_toolset = "default"`. (Keep the old value instead if you would rather
select this toolset explicitly with `--toolset backend`.)

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

`trust .` and `secret set` both need a terminal, and say so rather than guessing:
`refusing to trust: stdin is not a terminal` and `secret set needs a terminal to
prompt for the value`. The non-interactive forms are:

```bash
agentstack trust --preview                  # JSON review surface; read `surface_digest` from it
agentstack trust . --yes --consented sha256:<the surface_digest you just reviewed>
agentstack secret set GITHUB_TOKEN --value <VALUE>
```

`trust --preview` emits JSON on its own (there is no `--json` flag) and its
`surface_digest` value already carries the `sha256:` prefix; the grant refuses
unless that digest still matches the bytes on disk. Inline `--value` can land in
shell history — prefer the prompt when you have a terminal — and add
`--env-file` to write the project `.env` instead of the OS keychain.

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
