# agentstack Killer Feature Ideas

Date: 2026-06-25

## Core Bet

agentstack should not be positioned as only a config manager. The stronger product category is:

> Agent capability orchestration.

That means agentstack knows which capabilities exist, when they should be active, where they should render, whether they are safe, and how to prove the whole setup is correct.

The winning product is not "I sync MCP configs." It is:

> I make agent environments reproducible, minimal, safe, and task-aware.

## 1. Smart Profiles

Today, profiles are explicit:

```sh
agentstack use backend
```

The smarter version is:

```sh
agentstack activate
agentstack activate --explain
```

agentstack inspects the current repo and activates the smallest useful profile.

Signals it can use:

- `package.json`, `vite.config.*`, `next.config.*` -> frontend profile
- `pyproject.toml`, notebooks, `requirements.txt` -> Python/data profile
- `terraform/`, `infra/`, `pulumi.*` -> infra profile
- `.github/workflows/` -> CI/debug profile
- Prisma, SQL migrations, Diesel, Alembic -> database profile
- Figma links in docs or tickets -> design review profile
- Jira/Linear ticket IDs in branch name -> product/issue profile
- Docker/Kubernetes files -> platform profile

Example output:

```text
Activated profile: frontend

Reason:
- package.json found
- vite.config.ts found
- Figma links found in README.md

Enabled:
- github
- figma
- frontend-review skill

Skipped:
- postgres, no database files detected
- terraform, no infrastructure files detected
```

Why this matters:

- It solves context bloat.
- It makes profiles easier for normal developers.
- It turns agentstack from a static manifest renderer into an adaptive environment manager.

## 2. Capability Firewall

This is probably the most commercially valuable feature.

Every server, skill, instruction bundle, or recipe should get a risk profile:

```text
github
source: catalog
runs local code: yes, npx
needs secrets: GITHUB_TOKEN
network access: yes
filesystem access: no
pinning: floating package
status: approved
```

Policy could support rules like:

```toml
[policy]
forbid_local_exec = false
require_pinned_git = true
forbid_unreviewed_filesystem = true
allowed_sources = ["catalog:*", "git:github.com/acme/*", "registry:io.modelcontextprotocol/*"]
```

Useful risk labels:

- remote HTTP
- local executable
- package manager command
- needs secrets
- filesystem capable
- browser capable
- shell capable
- unpinned git source
- unpinned npm/pypi package
- source outside allowlist
- installed but never used

Why this matters:

- Teams do not just need convenience; they need governance.
- This gives security teams a reason to approve agentstack.
- It turns `doctor --ci` into a real enterprise adoption hook.

## 3. `agentstack explain`

Add a first-class explanation/debug command:

```sh
agentstack explain github
agentstack explain --target codex
agentstack explain --profile backend
```

It should answer:

- where the capability came from
- which profiles include it
- which targets it renders to
- whether it is active globally or per project
- what files it writes to
- what secrets it needs
- whether those secrets resolve
- whether policy allows it
- whether it is installed and locked
- the exact rendered config per harness

Example:

```text
github
Source: catalog
Type: stdio
Command: npx -y @modelcontextprotocol/server-github
Secrets: GITHUB_TOKEN resolved from keychain
Profiles: backend, ci-debug
Targets: codex global, claude-code project
Policy: allowed
Risk: local executable, network, secret
```

Why this matters:

- It answers "why is this available here but not there?"
- It makes agentstack easier to trust.
- It reduces support/debugging friction.

## 4. One-Command Team Bootstrap

For teams, the magic workflow should be:

```sh
agentstack bootstrap
```

It should:

1. detect installed harnesses
2. install locked skills
3. show missing harnesses
4. show missing secrets
5. guide secret setup
6. preview live config diffs
7. ask for confirmation
8. apply
9. run doctor

This replaces onboarding docs like:

> Install Claude, install Codex, copy this MCP block, put this token here, install these skills, edit this settings file.

Why this matters:

- This is a clear team adoption story.
- It makes a repo self-service.
- It gives agentstack a daily-use workflow, not just a maintenance workflow.

## 5. Capability Recipes

Do not start with a broad marketplace. Start with recipes.

Examples:

```sh
agentstack add recipe github-ci-review
agentstack add recipe frontend-design-review
agentstack add recipe incident-response
agentstack add recipe jira-product-engineering
agentstack add recipe data-analysis
```

A recipe can include:

- MCP servers
- skills
- instructions
- settings
- profile membership
- policy hints
- required secrets
- health checks

Example recipe:

```toml
[recipes.github-ci-review]
servers = ["github"]
skills = ["ci-debug", "pull-request-review"]
instructions = ["repo-review-rules"]
settings.codex.model_reasoning_effort = "high"
required_secrets = ["GITHUB_TOKEN"]
```

Why this matters:

- Real workflows need bundles, not one MCP server.
- Recipes are easier to explain than marketplaces.
- Teams can create internal recipes.

## 6. Repo Contracts

Let repositories declare agent requirements:

```toml
[contracts.default]
profiles = ["backend"]
required_servers = ["github", "postgres"]
required_skills = ["migration-review", "code-review"]
required_instructions = ["team-rules"]
```

Then:

```sh
agentstack doctor --contract
```

or:

```sh
agentstack bootstrap --contract
```

Why this matters:

- The repo becomes self-describing.
- New developers know exactly what their agent needs.
- CI can validate that committed agent setup is internally consistent.

## 7. Agent-Generated Setup, Human-Approved

The existing `agentstack mcp` mode can become a standout feature.

An agent working on a task could say:

```text
I need Figma access to inspect the design.

Proposed agentstack change:
- add figma MCP server
- add FIGMA_TOKEN reference
- apply to codex and claude-code
- no live config written
```

Then the human runs:

```sh
agentstack proposal show
agentstack proposal accept
agentstack apply --write
```

Why this matters:

- Agents get a safe path to request tools.
- Humans retain control.
- The manifest captures the change for review.

## 8. Drift Timeline

Today drift can be detected. The better version is a timeline:

```sh
agentstack history
```

Example:

```text
2026-06-25 18:40 applied backend profile to codex
2026-06-25 18:41 wrote ~/.codex/config.toml
2026-06-25 19:02 detected hand edit in ~/.codex/config.toml
2026-06-25 19:10 restored claude-code backup
```

Why this matters:

- Users trust tools more when they can see what happened.
- Debugging team setup gets easier.
- It creates an audit trail without polluting target configs.

## 9. Capability Health Checks

`doctor --live` should eventually prove that a capability works, not just that it handshakes.

Example:

```toml
[servers.github.health]
kind = "mcp_tool"
tool = "search_repositories"
args = { query = "agentstack" }

[servers.postgres.health]
kind = "mcp_tool"
tool = "query"
args = { sql = "select 1" }
```

Then:

```sh
agentstack doctor --live
```

can report:

```text
github     OK     handshake + 22 tools + test call passed
postgres   FAIL   secret resolves, connection refused
```

Why this matters:

- Developers care if the tool actually works.
- Teams can catch broken tokens or network access early.
- It makes `doctor` a real readiness check.

## 10. Minimal Context Mode

Add task-aware minimization:

```sh
agentstack minimize --task "fix failing CI"
```

Example output:

```text
Recommended profile: ci-debug

Enable:
- github
- ci-debug skill
- repo-instructions

Disable:
- figma
- postgres
- browser

Reason:
- branch contains "fix-ci"
- .github/workflows exists
- no DB migration files changed
```

Why this matters:

- Developers increasingly care about agent context quality.
- It reduces tool noise.
- It gives agentstack a smarter reason to exist beyond setup.

## 11. Capability Diff For Pull Requests

When a PR changes `agentstack.toml`, generate a human-readable capability diff:

```sh
agentstack diff --capabilities
```

Example:

```text
Added:
- linear: remote HTTP, needs LINEAR_TOKEN
- deploy-prod skill: git source, local instructions

Changed:
- github now active in frontend profile

Risk changes:
- new secret required: LINEAR_TOKEN
- new local executable: none
```

Why this matters:

- Agent setup becomes reviewable like dependencies.
- Security/platform reviewers can quickly understand impact.
- This is better than reviewing raw TOML.

## 12. Internal Capability Catalog

For companies, support a private catalog:

```toml
[providers.company]
type = "git"
url = "git@github.com:acme/agent-capabilities.git"
```

Then:

```sh
agentstack search kibana
agentstack add from acme/kibana
```

Why this matters:

- Internal MCP servers are probably where real company value is.
- Teams need curated, approved capabilities.
- This avoids relying on a public marketplace too early.

## Recommended Build Order

### First

Build features that strengthen trust and daily usability:

1. `agentstack explain`
2. unresolved-secret blocking before writes
3. capability risk scoring
4. capability diff for PRs

### Second

Build smart orchestration:

1. smart profile activation
2. repo contracts
3. one-command bootstrap
4. richer live health checks

### Third

Build ecosystem features:

1. recipes
2. private catalogs
3. agent-generated proposals
4. drift timeline

## Best Single Killer Feature

If only one feature can be built, build:

```sh
agentstack explain
```

Then expand it into:

```sh
agentstack explain --risk
agentstack explain --rendered
agentstack explain --why-active
agentstack explain --json
```

Why this one first:

- It uses existing data structures.
- It makes the current product easier to understand.
- It improves trust immediately.
- It supports future dashboard, CI, and agent workflows.

## Final Product Direction

The best version of agentstack is:

> A reproducible, policy-aware, task-aware capability layer for AI development environments.

The product should feel like:

- `cargo` for agent capabilities
- `terraform plan` for agent config changes
- `doctor` for readiness
- `explain` for trust and debugging
- smart profiles for context control

That combination is special.

