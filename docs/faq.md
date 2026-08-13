<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# FAQ

## What belongs in my central library?

Reusable skills, MCP server definitions, instructions, hooks, extensions, and
packages. It is a normal Git repo with a `library.toml` index and folders such
as `skills/`, `servers/`, and `instructions/`.

Do not put secret values, trust state, audit logs, or machine-specific CLI
configuration there. See [Central library](library.md).

## What belongs in each project repo?

Usually only `.agentstack/agentstack.toml` and
`.agentstack/agentstack.lock`. The manifest names the capabilities and default
toolset. The lock pins the exact library content that was reviewed.

Native MCP configs and copied skill folders are generated output or unnecessary
in the zero-files lane. Do not commit them as a second source of truth.

## What does the lock mean?

It freezes the exact resolved skill bodies, MCP definitions, instructions, and
other selected content, so a central library may move forward without silently
changing a project that already has a lock. Accept an intentional update by
writing a new lock, then reviewing it again — see
[Trust a project](howto/trust-a-repo.md).

## When do I lock again?

After you change what a toolset or project selects, or when you intentionally
accept updated library content. Unrelated project edits and new library commits
need no new lock. See [When to lock again](library.md#when-to-lock-again).

## Can I link my GitHub library and keep my old local library?

Yes. AgentStack combines all linked sources and the first one holding a kind and
name wins; existing locked projects keep their pinned content until deliberately
re-locked. See
[Several libraries work together](library.md#several-libraries-work-together)
for collisions and qualified names such as `local:rust-testing`.

## How does the agent see my skills if they are not copied into the repo?

The gateway gives the agent a compact index of loadable skill names and one-line
descriptions, and the agent loads one full body with
`agentstack_load(name, reason)` when the task matches — so the user never has to
name the skill. See [Dynamic skill loading](concepts.md#dynamic-skill-loading).

## Does every skill body enter the model's context?

No. Only the compact name-and-description index is present initially. Full
instructions load on demand and are recorded with the reason.

## How are MCP tools loaded?

The default compact gateway does not advertise every upstream schema at once.
It exposes the MCP servers selected by the toolset behind `tools_search`. The
agent searches for a needed tool and receives that tool's schema only when it
needs to call it.

## Do I need to run `agentstack use` before opening an agent?

Not in the normal zero-files workflow. A trusted new connection automatically
opens the declared default toolset. `use --write` is for file-only CLIs and
intentional rendered compatibility lanes.

If several toolsets exist without a default, AgentStack stays on control-plane
tools until one is selected. Set one with:

```bash
agentstack toolset default backend
agentstack toolset default backend --write
```

## When does a changed default reach an agent that is already running?

A modern MCP connection re-derives the trusted default from the project on its
next request, so it picks the change up without reconnecting. Only a legacy
connection keeps the selection it opened with; reconnect that one to receive
the new default.

## How do CLI- and model-specific instructions work?

An instruction fragment can have base, CLI, model, and CLI-plus-model bodies.
The most specific matching body wins. AgentStack uses a model explicitly named
by a toolset or managed setting and never guesses an unknown model.

Put reusable variants in
`instructions/<name>/instruction.toml` in the central library. See the
[example](library.md#put-cli--and-model-specific-instructions-in-the-library).

## Will AgentStack overwrite my existing CLI configs?

Installing AgentStack changes nothing by itself. `init` discovers existing MCP
entries and supported settings, then shows an import review. Accepted MCP
definitions are copied into the central library (`~/.agentstack/lib`, or your
first linked source) by default; the original CLI entries are not deleted. Every write has a preview or confirmation, managed
regions preserve surrounding user content, and recorded writes can be reviewed
with `agentstack undo`.

When the live gateway can carry skills and MCP servers, AgentStack leaves their
native project files absent.

## What happens to skills I already installed in Codex or another CLI?

They stay where they are and keep working through that CLI. `init` does not
automatically import or delete native skills. When you want one to become a
portable AgentStack skill, preview and adopt it explicitly:

```bash
agentstack adopt --to-library
agentstack adopt --to-library --write
```

The first command shows what would be adopted. The second stores accepted
skills in the central library so projects can select them by name.

## What about Computer Use and other app-owned tools?

AgentStack leaves them with the application that installed and updates them.
During `init`, MCP servers whose executable lives inside another app bundle are
named but excluded by default. This avoids copying one application's private
plumbing into unrelated CLIs or re-locking every time that app updates.

Use `agentstack init --include-tool-managed` only when you deliberately want to
override that boundary. Built-in CLI features, plugins, and connector systems
that are not native MCP config entries are not replaced by AgentStack.

## Where should my custom capabilities go?

Put reusable skills, MCP server definitions, and instructions in the central
library. A project normally contains only the manifest and lock that select
those names. Keep a capability inline or under `.agentstack/` only when it is
specific to that one repository.

## Why are empty folders still present after uninstall?

AgentStack removes empty parent folders it owns after removing its managed
files. It keeps non-empty folders and any folder containing user-owned content.
Run `agentstack doctor` if an empty managed folder remains; do not delete
provider trees blindly.

## Is my API key stored in the manifest or library?

No. Portable files contain `${REF}` placeholders. Each machine stores the
value with `agentstack secret set REF`, normally in the OS keychain. AgentStack
never puts resolved values in the lock, agent context, or audit log.

## Why must I trust my own project?

The same rule protects every path, including a repo you just cloned or changed.
Trust is a review of the current manifest and lock on one machine. A changed
consent surface becomes inert until reviewed again.

Trust means reviewed, not proven safe. Use policy and protected runs for
enforcement.

## Why can't my agent grant trust for me?

Because then the review would be a formality. The host guard refuses the
consent verbs — `trust`, `yes`, `init --yes`, `apply --yes` — when they come
from an agent shell, in every spelling, and says so:

```text
blocked: `agentstack trust` grants consent — it was refused
  nothing was granted · consent is granted at your terminal, not from an agent
  shell · the agent may prepare the review with `agentstack trust --preview`
```

The agent can still do the reading: `trust --preview` and `trust --list` are
allowed, so it can assemble the surface and explain it. You answer it in your
own terminal.

## Can my teammate use a different CLI than me?

Yes. The project declares intent once. Each adapter delivers it in the form its
CLI supports, and `agentstack up` connects only the CLIs detected on that
machine.

## Does AgentStack depend on T3 Code?

No. AgentStack works when a CLI starts directly or under an unchanged T3 Code
installation or another supervisor. T3 Code can control remote sessions;
AgentStack independently manages the portable capability configuration on each
machine.

## Does it work offline?

Already-pinned local content can work offline. New Git-backed content,
remote MCP servers, library pulls, and first-time downloads still need a
network. Use an intentional rendered lane when a CLI must work without the live
gateway.

## What should I run when something is wrong?

Start with:

```bash
agentstack status
```

It gives one next action. Use `agentstack doctor` for the deeper pass. Follow
the first fix it prints rather than editing generated provider files.

## When should I not use AgentStack?

If you have one CLI, one permanent configuration, and no need to share,
version, audit, or reuse it, native configuration may be simpler. AgentStack is
most useful when projects, CLIs, machines, or teammates multiply.

Next: [Get started](start.md) · [Central library](library.md) ·
[Troubleshooting](troubleshooting.md)
