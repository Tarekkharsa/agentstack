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
| Want it reusable across projects by name | `agentstack lib add-server` + reference it from a [profile](../concepts.md) |

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

# 4. Reusable across projects: store it in the library, then name it in a profile
agentstack lib add-server kibana --file ./kibana.toml --write
#   then in the manifest:  [profiles.backend]  servers = ["kibana"]

# After any of them: re-lock, then render into every CLI
agentstack lock
agentstack apply --write
```

Verbs 1–4 write only the [manifest](../concepts.md) (verb 4 also writes the
[central library](../concepts.md)) — commit-safe, with secrets kept as
`${REF}` placeholders. Nothing reaches a CLI until `apply --write` renders it.
Hand-edit `[servers.<name>]` in the manifest directly only when you need fields
the flags don't cover — native per-adapter keys under `extra.<adapter>`, a
launch `cwd`, `targets` scoping, or `owner`. Whenever you change a profile's
server list, re-lock with `agentstack lock` so the [lockfile](../concepts.md)
pins the new set, then `apply --write` to render.

**Limits.** Adding a server does not store its secret, trust it, or run it.
Store the value with `agentstack secret set GH_PAT` (it stays out of the
manifest). In the [zero-files delivery mode](trust-a-repo.md), a new server also
stays inert until you re-run `agentstack trust .`, because the edit changes the
[manifest digest](../concepts.md#trust-and-the-consent-digest).

- [Concepts](../concepts.md) — server, profile, library, secrets
- [Reference: `adopt` and `add`](../reference.md#adopt-and-add)
- [Reference: the central library](../reference.md#the-central-library)
