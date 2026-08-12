<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Tutorial

The walkthrough now lives in one place: **[Get started](start.md)**. It runs
the whole path end to end — install, link a library, put reusable capabilities
in it, keep a project small, lock and trust, and bootstrap another machine —
with real captured command output at each step.

This page is the map. Each step links to the section that teaches it.

| Step | Where it is taught |
| --- | --- |
| 1. Set up this machine | [Install and set up this machine](start.md#1-install-and-set-up-this-machine) |
| 2. Link your reusable library | [Link your central library](start.md#2-link-your-central-library) |
| 3. Put reusable skills and servers in it | [Put reusable capabilities in it](start.md#put-reusable-capabilities-in-it) |
| 4. Select names in a project | [Keep each project small](start.md#3-keep-each-project-small) |
| 5. Pin and review it | [Lock, review, done](start.md#4-lock-review-done) |
| 6. Start your agent normally | [What the agent sees](start.md#5-what-the-agent-sees) |
| 7. Bootstrap another machine | [Set up another machine](start.md#6-set-up-another-machine) |

Deeper on one topic:

- [Add an MCP server](howto/add-a-server.md) — one reusable definition, secrets
  by reference.
- [Add a skill](howto/add-a-skill.md) — author it once, select it by name.
- [Name a toolset](howto/name-a-toolset.md) — the command form of a toolset,
  and how a default is chosen.
- [Trust a project](howto/trust-a-repo.md) — why the lock comes first, and what
  the review actually shows.
- [Undo AgentStack changes](howto/undo.md) — the timeline, and taking a managed
  write back.
- [Team setup](howto/team-setup.md) — the same library across people and CI.

Next: [Get started](start.md) · [Central library](library.md) ·
[FAQ](faq.md) · [Full reference](reference.md)
