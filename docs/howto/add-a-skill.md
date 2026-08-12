<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Add a skill

For a skill you want across projects, put it in the central library.

## Create one

```bash
agentstack lib new api-review
```

Edit `api-review/SKILL.md`. The frontmatter description matters because it is
the small routing hint every agent sees before the full body is loaded:

```markdown
---
name: api-review
description: Review API changes for compatibility, error handling, and migration risk. Use when a task adds or changes a public endpoint, request, response, or schema.
---

# Review an API change

1. Inspect the public contract and its callers.
2. Check backward compatibility and failure behavior.
3. Report concrete findings before proposing broad redesign.
```

Keep the body focused. Put long examples in a `references/` folder and link
only the ones the agent should open for a matching task.

## Add it to the library

```bash
agentstack lib add ./api-review
agentstack lib add ./api-review --write
agentstack lib list
```

The first command previews the destination, content scan, and provenance. New
content goes into the first writable linked source.

Import from Git in the same way:

```bash
agentstack lib add anthropics/skills --list
agentstack lib add anthropics/skills --skill pdf
agentstack lib add anthropics/skills --skill pdf --write
```

## Select it in a project

```toml
[toolsets.backend]
skills = ["api-review"]
```

Then preview, pin, and review:

```bash
agentstack lock          # preview
agentstack lock --write
agentstack trust .
```

On the next trusted connection the agent sees `api-review` plus its one-line
description. It calls `agentstack_load` only when a task matches. No copied
project skill folder or `agentstack use` step is needed in the live lane.

## Keep a skill inside one project instead

Use a project-local skill when it makes sense only with that repository. The
guided quick path is:

```bash
agentstack yes
```

It reviews undeclared content under `.agentstack/skills/`, then declares,
pins, and activates approved files through one human confirmation. Content
from an untrusted clone is not eligible for this shortcut.

Next: [Central library](../library.md) · [Trust](trust-a-repo.md) ·
[Skill reference](../reference.md#add-skill-source--install-from-any-skills-repo)
