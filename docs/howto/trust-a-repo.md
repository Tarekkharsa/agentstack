<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Trust a project

A new clone is inert. Its AgentStack gateway exposes control-plane tools only:
no project MCP server starts, no remote server is contacted, no project skill
body is loaded, and no secret resolves.

## Review it

```bash
agentstack status     # inspect first
agentstack trust .
```

The review shows what the manifest and lock allow: commands, remote endpoints,
secret reference names, skill and instruction content, and machine policy that
still limits the project.

Trust is bound to this checkout path and the current manifest plus lock. A
`git pull`, manifest edit, or re-lock makes it stale.

## Lock first, then trust

When selected content changed:

```bash
agentstack lock          # preview
agentstack lock --write
agentstack trust .
```

The lock preview is the content decision. Trust is the human consent decision.
Doing them in reverse wastes the trust review because writing a new lock changes
the approved surface.

## What becomes available

After trust, a new gateway connection opens the project's default toolset.
Skills and MCP servers arrive live where supported. Instructions, settings,
hooks, extensions, and file-only compatibility lanes use the managed files the
CLI requires.

You do not need `agentstack use` to activate the normal zero-files lane.

## What trust does not mean

Trust means “I reviewed these declared bytes and references.” It does not prove
that third-party code is safe, isolate a process, or let a project exceed
machine policy.

For stronger runtime control use `agentstack run <cli>`, `--sandbox`, or
`--lockdown` as appropriate.

## Consent does not travel

Another teammate, machine, checkout, worktree, or CI runner reviews for itself.
Share the project manifest and lock; never share the trust store or secret
values.

Useful commands:

```bash
agentstack trust --list
agentstack trust --revoke
agentstack x explain <capability>
```

Removing AgentStack-managed content remains possible without trust. A stale
project should never trap you into keeping generated output.

Next: [Project files and lock](../concepts.md#the-manifest-and-the-lockfile) ·
[Team setup](team-setup.md) · [Enforcement limits](../ENFORCEMENT.md)
