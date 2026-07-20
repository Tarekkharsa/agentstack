<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Add a skill

For anyone giving their agent CLIs a skill — a portable directory with a
`SKILL.md` of instructions. Prerequisite: a project with an
`.agentstack/agentstack.toml` [manifest](../concepts.md) (run
`agentstack init` if you don't have one).

Four verbs, by where the skill lives and how long you want it:

| You have | Use |
| --- | --- |
| Any skills repo (GitHub shorthand, git URL) or a local dir | `agentstack add skill <source>` |
| A skill you want reusable across projects by name | `agentstack lib add <source>` + reference it from a [profile](../concepts.md) |
| Nothing yet — you're writing one | `agentstack lib new <name>` scaffolds the template |
| Curiosity — run it once, install nothing | `agentstack try <source> \| <your agent CLI>` |

```bash
# 1. From the ecosystem: inspect, preview, then write
agentstack add skill anthropics/skills --list
agentstack add skill anthropics/skills --skill pdf          # dry run: scan + diff + digest
agentstack add skill anthropics/skills --skill pdf --write

# Sources: owner/repo, owner/repo@skill, tree URLs, git remotes, ./local-dir
agentstack add skill https://github.com/o/r/tree/main/skills/pdf --write
agentstack add skill ./my-skill --write

# 2. Reusable across projects: into the central library, then name it in a profile
agentstack lib add anthropics/skills --skill pdf --write
#   then in any manifest:  [profiles.backend]  skills = ["pdf"]

# 3. Author one from scratch
agentstack lib new code-review        # scaffolds ./code-review/SKILL.md
#   edit it, then adopt with verb 1 or 2

# 4. Try before anything: ephemeral, manifest-free
agentstack try anthropics/skills --skill pdf | claude
```

Every source is content-scanned (hidden-unicode / prompt-injection) before
anything is offered, and a dry run fetches into transient staging — the
[manifest](../concepts.md), [lockfile](../concepts.md), and content store
stay untouched until `--write`. The write records the exact commit and
content checksum in the lockfile, and — in the static delivery mode, when
the active profile is unambiguous — materializes the skill into your CLIs'
skills directories immediately. Other modes get the honest next step
printed: `session start` for clean-at-rest; `agentstack trust .` for
[zero-files](trust-a-repo.md), because the manifest edit re-gates trust.

**Limits.** Adding a skill never runs it, and a scan finding that blocks is
a decision for you, not a flag to reach for reflexively
(`--allow-flagged` admits it with the warnings on record). Skill names obey
one contract — lowercase `[a-z0-9._-]`, 64 chars — and a source directory
that doesn't fit gets `--name` to choose. Update later with
`agentstack lock --update` (branch and rev-less pins re-track their
upstream; a vanished repo errors instead of pretending).

- [Concepts](../concepts.md) — skill, profile, library, lockfile
- [Reference: `add skill <source>`](../reference.md#add-skill-source--install-from-any-skills-repo)
- [Reference: the central library](../reference.md#the-central-library)
