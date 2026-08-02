<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Add a skill

For anyone giving their agent CLIs a skill — a portable directory with a
`SKILL.md` of instructions. Prerequisite: a project with an
`.agentstack/agentstack.toml` [manifest](../concepts.md) (run
`agentstack init` if you don't have one).

## Writing one yourself: drop the file, then say yes

Write the directory where skills live, then answer one question:

```bash
mkdir -p .agentstack/skills/pdf-review
$EDITOR .agentstack/skills/pdf-review/SKILL.md   # your instructions

agentstack yes
```

> `agentstack yes` is v0.18.0 and later; the current stable install serves
> v0.17.1, where a skill has to be declared in the manifest before it renders.
> `agentstack --version` says which you have.

Until you answer, the file is **inert** — nothing resolves, pins, renders, or
reads it, and no agent sees it. `agentstack yes` shows one review: what will be
declared, what will be pinned, what each CLI will get, and the full consent
surface. One confirmation makes it live everywhere. Undo is
`agentstack restore --last --write`, named in the preview before it runs.

The short path is for content you demonstrably wrote here — untracked in git,
or newer than this project's last review. A skill that **came with the
repository** is somebody else's work and takes the full staged review instead;
`agentstack yes` says so and names the commands (`agentstack adopt` →
`agentstack lock` → `agentstack trust .`). `agentstack lib new <name>`
scaffolds the template if you'd rather not start from an empty file.

## Bringing in someone else's skill

| You have | Use |
| --- | --- |
| Any skills repo (GitHub shorthand, git URL) or a local dir | `agentstack add skill <source>` |
| A skill you want reusable across projects by name | `agentstack lib add <source>` + reference it from a [toolset](../concepts.md) |
| Curiosity — run it once, install nothing | `agentstack try <source> \| <your agent CLI>` |

```bash
# 1. From the ecosystem: inspect, preview, then write
agentstack add skill anthropics/skills --list
agentstack add skill anthropics/skills --skill pdf          # dry run: scan + diff + digest
agentstack add skill anthropics/skills --skill pdf --write

# Sources: owner/repo, owner/repo@skill, tree URLs, git remotes, ./local-dir
agentstack add skill https://github.com/o/r/tree/main/skills/pdf --write
agentstack add skill ./my-skill --write

# 2. Reusable across projects: into your first linked library source, then name it in a toolset
agentstack lib add anthropics/skills --skill pdf --write
#   then in any manifest:  [profiles.backend]  skills = ["pdf"]

# 3. Author one from scratch (see "drop the file, then say yes" above)
agentstack lib new code-review        # scaffolds ./code-review/SKILL.md

# 4. Try before anything: ephemeral, manifest-free
agentstack try anthropics/skills --skill pdf | claude
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
`agentstack lock --update` (branch and rev-less pins re-track their
upstream; a vanished repo errors instead of pretending).

- [Concepts](../concepts.md) — skill, toolset, library, lockfile
- [Reference: `add skill <source>`](../reference.md#add-skill-source--install-from-any-skills-repo)
- [Reference: the library](../reference.md#the-library-linked-source-folders)
