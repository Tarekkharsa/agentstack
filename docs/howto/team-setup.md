<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Share one setup with a team

Share two normal Git repos:

1. A central library repo containing reusable skills, servers, and
   instructions.
2. Each project repo containing `.agentstack/agentstack.toml` and
   `.agentstack/agentstack.lock`.

## Maintainer

```bash
git -C ~/GitHub/ai-setup push
git add .agentstack/agentstack.toml .agentstack/agentstack.lock
git commit -m "Configure AgentStack"
git push
```

Commit intent and pinned content, never credentials or generated provider
configuration. Server definitions contain `${REF}` placeholders.

## Teammate or another machine

After cloning the project:

```bash
agentstack up --library https://github.com/your-team/ai-setup.git
agentstack up --library https://github.com/your-team/ai-setup.git --write
agentstack status
```

Status names any secret references this machine still needs and the local trust
review. Each person then runs the commands it shows, for example:

```bash
agentstack secret set GITHUB_TOKEN
agentstack trust .
agentstack status
```

Trust never travels between people, machines, or checkout paths. That is why a
cloned project stays inert until its local review.

## Update the shared setup

When library content changes, teammates refresh it with `agentstack up`. A
project continues serving its locked version until its maintainer deliberately
re-locks and commits the changed lock:

```bash
agentstack up            # preview
agentstack up --write
agentstack lock          # preview
agentstack lock --write
agentstack trust .
git add .agentstack/agentstack.lock
git commit -m "Update AgentStack capabilities"
git push
```

After pulling that commit, every teammate reviews the changed consent surface
with `agentstack trust .`. Their secret values remain local.

Team members may use different supported CLIs. AgentStack routes live
capabilities to MCP-capable tools and writes only the kinds or compatibility
lanes that require files.

Next: [Central library](../library.md) · [Trust a clone](trust-a-repo.md) ·
[Use it in CI](ci.md)
