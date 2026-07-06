---
name: sync-library
description: Keep your agentstack central library (~/.agentstack/lib) consistent across machines by versioning it as a git repo — commit the index and path-source skill bodies, exclude the content store cache and any resolved secrets.
---

# Sync the central library

Use when you want your agentstack central library — the skills and MCP servers
you reference by name — to be the same on every machine (laptop, desktop, a
fresh box).

## The idea

The library lives at `~/.agentstack/lib/`. Make that directory a git repo and
push/pull it. git is the right tool here: versioned, offline-friendly, no
daemon, and it diffs cleanly.

## What to commit vs. exclude

Commit:

- `library.toml` — the index (names, provenance, checksums)
- `skills/` — path-source skill bodies (the files you installed by name)
- `servers/` — server definitions (`${REF}` placeholders only, never resolved
  secret values)

Exclude (put in the repo's `.gitignore`):

- the content **store / cache** (git-source clones) — large and re-fetchable
- anything holding a resolved secret value

Git-source skills carry `git:<url>@<rev>#<subpath>` provenance, so they
re-resolve identically on any machine — only path-source content actually needs
to travel.

## First-time setup

```bash
cd ~/.agentstack/lib
printf 'store/\n*.local\n' >> .gitignore
git init && git add library.toml skills servers .gitignore
git commit -m "library snapshot"
git remote add origin <your-remote>
git push -u origin main
```

## On another machine

```bash
# fresh machine:
git clone <your-remote> ~/.agentstack/lib

# existing machine, pull latest:
cd ~/.agentstack/lib && git pull
```

## Secrets stay per-machine

Server definitions travel with `${REF}` placeholders, never values. On a new
machine, set the secrets locally and let doctor tell you what's missing:

```bash
agentstack secret set <NAME>
agentstack doctor
```

## Keep it clean

- Don't commit the store cache — it bloats history and re-fetches anyway.
- The index is name-keyed and sorted, so cross-machine edits rarely conflict; if
  they do, it's an ordinary git merge of `library.toml`.
