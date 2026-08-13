<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Central library

Your central library is a normal folder, usually a Git repo, containing the
capabilities you want to reuse across projects and machines. AgentStack does
not require a hosted AgentStack account or server.

## Recommended folder structure

```text
ai-setup/
├── library.toml
├── skills/
│   └── rust-testing/
│       └── SKILL.md
├── servers/
│   └── upstash-context7.toml
├── instructions/
│   └── team-style/
│       ├── instruction.toml
│       ├── base.md
│       ├── codex.md
│       └── claude-opus.md
├── hooks/
├── extensions/
└── packages/
```

`library.toml` is the index AgentStack maintains. Keep secret values out of the
repo; server definitions use `${REF}` placeholders, and each machine stores its
own value with `agentstack secret set REF`.

## Link it once on each machine

```bash
agentstack lib link ~/GitHub/ai-setup --name central --first
agentstack lib link ~/GitHub/ai-setup --name central --first --write
agentstack lib sources
```

The link is machine-local. Projects only store capability names and their
locked digests, so your personal filesystem path never enters a project repo.

## Several libraries work together

AgentStack reads linked sources from first to last, like `PATH`:

```text
1. central  ~/GitHub/ai-setup
2. local    ~/.agentstack/lib
3. work     ~/Company/agent-library
```

- Different names form one combined library.
- The first source wins when the same kind and name appears twice.
- `agentstack lib sources` and `agentstack doctor` show every collision.
- Use a qualified name such as `local:rust-testing` when a project needs a
  shadowed copy.
- An inline project definition overrides a library item with the same name.

Changing source order affects the next lock. It does not change the exact bytes
an already-locked project serves.

## How a project uses the library

The project selects names in `.agentstack/agentstack.toml`:

```toml
version = 1
default_toolset = "backend"

[toolsets.backend]
servers = ["upstash/context7", "github"]
skills = ["api-review", "rust-testing"]

[instructions.team-style]
targets = ["*"]
```

There are no `[skills.api-review]` or `[servers.github]` definitions here.
That absence means "resolve this name from my linked libraries." The lock then
records the exact source and digest that won.

## Add reusable content

```bash
agentstack lib new api-review
agentstack lib add ./api-review
agentstack lib add ./api-review --write

agentstack lib add-server github --file ./github.toml
agentstack lib add-server github --file ./github.toml --write

agentstack lib list
```

New content goes to the first writable source. Every write has a preview form.
You can also edit a linked Git checkout normally and then use AgentStack to
validate, index, and lock the content.

## Put CLI- and model-specific instructions in the library

Use one instruction folder with a base body and optional variants:

```toml
# instructions/team-style/instruction.toml
path = "base.md"

[[variant]]
cli = "codex"
path = "codex.md"

[[variant]]
cli = "claude-code"
model = "opus"
path = "claude-opus.md"
```

Select it from the project without a path:

```toml
[instructions.team-style]
targets = ["codex", "claude-code"]

[toolsets.backend]
model = "opus"
servers = ["github"]
skills = ["api-review"]
```

The most specific match wins: CLI + model, then CLI, then model, then the base
body. AgentStack never guesses an unknown model. Every body is pinned, even a
variant that is not selected on the current machine.

Instructions are compiled into the global or project instruction channel each
CLI actually supports, such as managed regions in `AGENTS.md` or `CLAUDE.md`.
Skills and MCP servers use the live zero-files gateway where supported.

## When to lock again

Lock again after you intentionally change what the project selects, or when you
want it to accept updated library content. The preview shows exactly which
source and digest would change; an updated library alone never updates a locked
project silently. Re-locking changes the consent surface, so the project asks
for [trust](howto/trust-a-repo.md) again.

## Sync it to another machine

Use ordinary Git, or let the bootstrap flow handle it:

```bash
agentstack up --library https://github.com/you/ai-setup.git
agentstack up --library https://github.com/you/ai-setup.git --write
```

What travels: library definitions, project manifests, and project locks.
What stays local: secrets, trust, machine policy, audit history, and installed
CLI state.

Next: [Get started](start.md) · [Toolsets](howto/name-a-toolset.md) ·
[Full library reference](reference.md#the-library-linked-source-folders)
