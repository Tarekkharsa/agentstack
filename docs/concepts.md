<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Concepts

You only need three layers to understand AgentStack.

```text
CENTRAL LIBRARY       PROJECT                    THIS MACHINE
reusable content  →  manifest + lock        →  CLIs, secrets, trust
Git repo              committed with project    local state
```

## Central library

A normal folder, usually a Git repo, that holds reusable skills, MCP server
definitions, instructions, hooks, extensions, and packages. Projects reference
items by name; they do not copy the content.

One machine can link several libraries, read in order like `PATH`: the first
source holding a kind and name wins. See
[Several libraries work together](library.md#several-libraries-work-together)
for collisions, qualified names, and what changing the order affects.

## The manifest and the lockfile

A zero-files project normally commits only:

```text
.agentstack/
├── agentstack.toml
└── agentstack.lock
```

**Manifest** — `agentstack.toml` names what the project may use: toolsets,
servers, skills, instructions, settings, hooks, extensions, workflows, and
policy. It may contain `${REF}` secret placeholders but never secret values.

A **workflow** is one of those kinds: a reviewed script that fans a task out to
several governed agent runs. Its bytes are pinned in the lock and gated by
trust like every other kind, so it stays inert until you review it. See
[Governed workflows](workflows.md).

**Lock** — `agentstack.lock` records the exact resolved content and digest for
every selected library item. It makes an updated library an explicit project
decision instead of a silent runtime change.

The lock is part of the consent surface, so a new lock always comes before
`agentstack trust .`. See [Trust a project](howto/trust-a-repo.md) for that
ceremony.

Commit both files. Do not commit native MCP configs or copied skill folders in
the normal live workflow.

## Toolset and default toolset

A **toolset** is a named group for one task:

```toml
default_toolset = "backend"

[toolsets.backend]
servers = ["github", "postgres"]
skills = ["api-review", "sql-review"]
```

The default toolset opens automatically for a trusted gateway. If exactly one
toolset exists, AgentStack can use it as the effective default. If several
exist without a declared default, the gateway offers only control-plane tools
until the launcher supplies a selection or a legacy client opens a lease.

Modern MCP requests re-derive the reviewed default each time and do not hide
authority in a protocol session. Legacy connections keep the selection they
opened with for compatibility. A non-default modern selection belongs in the
launch context, not in invisible connection state.

## CLI, adapter, target

**CLI** or **harness** is the agent program: Codex, Claude Code, OpenCode, and
others. **Adapter** is AgentStack's description of that CLI's confirmed config,
instruction, hook, and gateway channels. **Target** is the adapter id a command
or manifest entry acts on.

## Delivery modes

AgentStack registers one global gateway in each MCP-capable CLI. When an agent
starts inside a trusted project, the gateway discovers that project and serves
its default toolset live.

| Capability | Normal delivery |
| --- | --- |
| Skills | Name and description first; full body on demand |
| MCP servers | Live behind the compact gateway |
| Instructions / house rules | Managed CLI instruction files |
| Settings, hooks, extensions | Managed files required by the CLI |
| File-only CLI | Rendered compatibility files |

“Zero files” means no generated project MCP configs or copied skill folders for
the live lane. The manifest, lock, and any required managed instruction files
still exist.

Use `agentstack use <toolset> --write` only for a file-only CLI or an intentional
rendered compatibility lane. A missing `.mcp.json` or `.claude/skills/` is not a
problem when status says the capability is served live.

Two commands are easy to confuse. `agentstack use <toolset> --write`
**materializes one toolset's files now** and records nothing about routing;
`agentstack more delivery render-locally --write` **records a durable preference**
that routes kinds to files from then on, for every toolset. One is an action,
the other is a setting.

## What AgentStack owns

AgentStack manages only content it declared or recorded: the manifest, lock,
its gateway registration, its managed instruction regions, and rendered
entries or skill folders in its ownership ledger.

It does not automatically absorb or delete every capability already present on
the machine. Existing native skills remain CLI-owned until you explicitly
adopt them. Existing MCP entries are copied only through the reviewed `init` or
`adopt` flow, and their source entries remain in place. App-owned tools such as
Computer Use and servers installed inside another application's bundle are
excluded from import by default. Unrelated settings, plugins, and built-in CLI
features remain native.

This gives one useful boundary: put capabilities you want to reuse and govern
in the central library; leave vendor- or application-owned capabilities with
their owner.

## Dynamic skill loading

The gateway places a compact skill index in the agent's initial context:

```text
api-review — Review API changes for compatibility and error handling
rust-testing — Plan and write focused Rust tests
using-agentstack — Operate an AgentStack-managed setup
```

It is names plus one-line descriptions, capped to keep context small. The agent
can refresh or search it with `agentstack_list_loadable(query)`. When a task
matches a description, it calls `agentstack_load(name, reason)` and receives
only that skill's full body.

The user may ask for a skill by name, but does not have to. A clear frontmatter
description tells the agent when the skill is relevant. The embedded
`using-agentstack` manual is always available, even before a project is trusted.

## Dynamic MCP tool discovery

The default gateway keeps upstream MCP schemas out of the initial tool list.
The agent calls `tools_search({ query })` to find a tool, then asks for that
tool's schema and invokes it by its namespaced name. This keeps the agent's
context bounded even when several MCP servers expose hundreds of tools.

Toolsets decide which MCP servers can be discovered. Machine and project policy
still apply to every brokered call.

## Instructions by CLI and model

Instructions are reusable fragments such as coding rules or response style.
They can have one base body and variants selected by CLI, model, or both:

```toml
[instructions.team-style]
path = "./instructions/base.md"

[[instructions.team-style.variant]]
cli = "codex"
path = "./instructions/codex.md"

[[instructions.team-style.variant]]
model = "opus"
path = "./instructions/opus.md"
```

The most specific match wins: CLI + model, CLI, model, then base. AgentStack
never guesses an unknown model. The model comes from a named toolset or a model
setting AgentStack manages for that CLI.

In the central library, use
`instructions/<name>/instruction.toml` with the same `[[variant]]` grammar. A
project selects the library fragment with a sourceless
`[instructions.<name>]` table. See [the complete example](library.md#put-cli--and-model-specific-instructions-in-the-library).

## Trust and the consent digest

Trust is the local human approval for one project's current manifest and lock.
A new clone is inert until reviewed. Editing either file, pulling a changed
version, or re-locking makes trust stale.

Trust means “I reviewed these declared capabilities.” It does not prove that
third-party code is safe and it does not copy approval to another machine.

The **consent digest** is the hash AgentStack records for that reviewed
manifest and lock. If either changes, the digest no longer matches and the
project becomes inert again.

## Secrets

Manifests and library definitions use references such as `${GITHUB_TOKEN}`.
Each machine stores its own value:

```bash
agentstack secret set GITHUB_TOKEN
```

AgentStack resolves values only when needed. It does not write them into the
library, project manifest, lock, agent context, or audit log. An unresolved
secret blocks the operation and names the missing reference.

## Policy, guard, and protected runs

**Machine policy** is the maximum authority allowed on this machine. A project
can narrow it, never widen it. It can restrict tools, filesystem paths, network
destinations, and which server may read each secret.

**Guard** is a pre-tool-use check for supported CLIs. It can stop dangerous
commands before a harness runs them.

**Protected run** — `agentstack run <cli>` verifies trust, lock, and policy,
then freezes the selected tool surface for the run. It is host-side gating, not
OS isolation. `--sandbox` adds a container; `--lockdown` forces network access
through the audited proxy.

## Egress

Egress is network access from a brokered server or protected run. Machine and
project policy can restrict destinations. In lockdown, the audited proxy is the
only route out; in an ordinary host run, AgentStack does not claim kernel-level
network isolation.

## Lease, session, or protected-run fence

A live **lease** selects one toolset for one MCP connection without writing
files. Connection-scoped toolset leases are **legacy-only**: a modern MCP
connection re-derives the trusted default on each request, so the binary
refuses to fence one for it. Pick a different toolset with
`agentstack toolset default <name> --write`, or per launch with
`agentstack run <cli> --toolset <name>`. A native **session** temporarily
renders a toolset for compatibility and restores it at the end. A **protected-run
fence** selects and freezes a toolset for one `agentstack run` process. The
normal declared default is enough for ordinary trusted connections.

## Drift

Drift is a difference between AgentStack's source of truth and a generated
native file.

- Keep the manifest: `agentstack apply` previews restoring its rendered output.
- Keep a deliberate native hand-edit: `agentstack adopt` previews importing it.

Do not repair drift by editing another generated file. Run `agentstack doctor`
and follow the exact first fix it reports.

## Machine manifest and machine policy

These remain local and are intentionally not shared through a library or
project repo:

- linked library paths
- installed and detected CLIs
- secret values
- trust grants
- machine policy
- active connections and leases
- audit history and undo records

`agentstack up --library <git-url>` reconstructs the portable part on another
machine, then tells you which local decisions remain.

## The practical loop

```text
edit library or project
        ↓
preview lock → write lock
        ↓
human trust review
        ↓
new agent connection opens the default
        ↓
status for the next action; doctor only when needed
```

Next: [Get started](start.md) · [Central library](library.md) ·
[FAQ](faq.md) · [Feature reference](reference.md)
