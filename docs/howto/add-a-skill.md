<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Add a skill

For anyone giving their agent CLIs a skill — a portable directory with a
`SKILL.md` of instructions. Prerequisite: a project with an
`.agentstack/agentstack.toml` [manifest](../concepts.md) (run
`agentstack init` if you don't have one).

## Writing one yourself: write it, declare it, approve it

Write the directory where skills live, then take it through the four steps that
turn a file on disk into a capability your CLIs can use:

```bash
mkdir -p .agentstack/skills/pdf-review
$EDITOR .agentstack/skills/pdf-review/SKILL.md   # your instructions

agentstack adopt --write     # declare it in the manifest
agentstack lock --write      # pin its bytes in agentstack.lock
agentstack trust .           # review the changed consent surface and approve it
agentstack use --write       # activate it
```

Until you approve it, the file is **inert** — nothing resolves, pins, renders,
or reads it, and no agent sees it. `agentstack trust .` shows the review: what
will run, what it will contact, what secrets it can read, and the bytes each
item is pinned to. Undo any of it with `agentstack x restore --last --write`.

`agentstack x lib new <name>` scaffolds the template if you'd rather not start
from an empty file.

In v0.18.0 and later, `agentstack yes` performs the same four steps behind one
review and one confirmation — through the same functions and the same gate. See
[newer than the stable release](../start.md#newer-than-the-stable-release).

## Bringing in someone else's skill

| You have | Use |
| --- | --- |
| Any skills repo (GitHub shorthand, git URL) or a local dir | `agentstack add skill <source>` |
| A skill you want reusable across projects by name | `agentstack x lib add <source>` + reference it from a [toolset](../concepts.md) |
| Curiosity — run it once, install nothing | `agentstack x try <source> \| <your agent CLI>` |

```bash
# 1. From the ecosystem: inspect, preview, then write
agentstack add skill anthropics/skills --list
agentstack add skill anthropics/skills --skill pdf          # dry run: scan + diff + digest
agentstack add skill anthropics/skills --skill pdf --write

# Sources: owner/repo, owner/repo@skill, tree URLs, git remotes, ./local-dir
agentstack add skill https://github.com/o/r/tree/main/skills/pdf --write
agentstack add skill ./my-skill --write

# 2. Reusable across projects: into your first linked library source, then name it in a toolset
agentstack x lib add anthropics/skills --skill pdf --write
#   then in any manifest:  [toolsets.backend]  skills = ["pdf"]

# 3. Author one from scratch (see "write it, declare it, approve it" above)
agentstack x lib new code-review        # scaffolds ./code-review/SKILL.md

# 4. Try before anything: ephemeral, manifest-free
agentstack x try anthropics/skills --skill pdf | claude
```

Every source is content-scanned (hidden-unicode / prompt-injection) before
anything is offered, and a dry run fetches into transient staging — the
[manifest](../concepts.md), [lockfile](../concepts.md), and content store
stay untouched until `--write`. The write records the exact commit and
content checksum in the lockfile, and — where the skill is
[routed to the rendered lane](../concepts.md#delivery-modes) and the active
toolset is unambiguous — materializes it into your CLIs' skills directories
immediately. Where it is served live instead, the honest next step is printed:
`agentstack trust .`, because the manifest edit re-gates trust
([trust a repo](trust-a-repo.md)); a clean-at-rest project gets
`session start`.

**Limits.** Adding a skill never runs it, and a scan finding that blocks is
a decision for you, not a flag to reach for reflexively
(`--allow-flagged` admits it with the warnings on record). Skill names obey
one contract — lowercase `[a-z0-9._-]`, 64 chars — and a source directory
that doesn't fit gets `--name` to choose. Update later with
`agentstack lock --update --write` (branch and rev-less pins re-track their
upstream; a vanished repo errors instead of pretending).

- [Concepts](../concepts.md) — skill, toolset, library, lockfile
- [Reference: `add skill <source>`](../reference.md#add-skill-source--install-from-any-skills-repo)
- [Reference: the library](../reference.md#the-library-linked-source-folders)
