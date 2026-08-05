<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Add a server

For anyone adding an MCP ([Model Context Protocol](../concepts.md) — the plugin
standard agent CLIs use for tools) server to their setup. Prerequisite: a
project with an `.agentstack/agentstack.toml` [manifest](../concepts.md) (run
`agentstack init` if you don't have one).

Four verbs add a server. Pick by what you already have:

| You have | Use |
| --- | --- |
| The server's config details (URL or command) | `agentstack add server` — or `set server` to overwrite one that exists |
| Just a name — find it in the catalog or registry | `agentstack search` → `agentstack add from <id>` |
| Already hand-added it to one CLI's config | `agentstack adopt --write` |
| Want it reusable across projects by name | `agentstack x lib add-server` + reference it from a [toolset](../concepts.md) |

```bash
# 1. Have the details: add (or set, to overwrite) the server
agentstack add server github --type http \
  --url https://api.githubcopilot.com/mcp/ \
  --header "Authorization=Bearer ${GH_PAT}" --write

# 2. Know only a name: find it, then add it
agentstack search github
agentstack add from github --write

# 3. Hand-added it to one CLI already: pull it back into the manifest
agentstack adopt --write

# 4. Reusable across projects: store it in a linked library source, then name it in a toolset
agentstack x lib add-server kibana --file ./kibana.toml --write
#   then in the manifest:  [toolsets.backend]  servers = ["kibana"]

# After any of them: re-lock, review the change, then render into every CLI
agentstack lock --write
agentstack trust .          # the manifest and lock both moved — approve them
agentstack apply --write
```

Verbs 1–4 write only the [manifest](../concepts.md) (verb 4 also writes your
first [linked library source](../concepts.md)) — commit-safe, with secrets kept as
`${REF}` placeholders. Nothing reaches a CLI until `apply --write` renders it.
Hand-edit `[servers.<name>]` in the manifest directly only when you need fields
the flags don't cover — native per-adapter keys under `extra.<adapter>`, a
launch `cwd`, `targets` scoping, or `owner`. Whenever you change a toolset's
server list, re-lock with `agentstack lock --write` so the [lockfile](../concepts.md)
pins the new set, review it with `agentstack trust .`, then `apply --write` to
render.

**Why the `trust .` step is there.** Adding a server changes the manifest, and
re-locking changes the lockfile — both are part of the
[consent surface](../concepts.md#trust-and-the-consent-digest), so the write
re-opens the review. Skip it and `apply --write` refuses out loud rather than
writing a server definition your CLI would launch on its own:

```text
✗ refusing to render MCP servers: project at /path/to/repo changed since it
  was trusted — review and `agentstack trust .` before writing server
  definitions the harness launches on its own ('github')
```

Do it in this order — `lock --write` first, then `trust .`. Re-locking after you
approve would invalidate the approval you just gave. See
[trust a cloned repo](trust-a-repo.md).

**Limits.** Adding a server does not store its secret, trust it, or run it.
Store the value with `agentstack secret set GH_PAT` (it stays out of the
manifest). The review gate is not confined to the live lane: a new server stays
inert until you re-run `agentstack trust .` whichever way it is delivered.

- [Concepts](../concepts.md) — server, toolset, library, secrets
- [Reference: `adopt` and `add`](../reference.md#adopt-and-add)
- [Reference: the library](../reference.md#the-library-linked-source-folders)
