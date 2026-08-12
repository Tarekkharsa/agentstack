# Reusable instructions

Keep a fragment in a linked source:

```text
instructions/team-style/
├── instruction.toml
├── base.md
├── codex.md
└── claude-opus.md
```

```toml
# instruction.toml
path = "base.md"

[[variant]]
cli = "codex"
path = "codex.md"

[[variant]]
cli = "claude-code"
model = "opus"
path = "claude-opus.md"
```

Select it by name from a project, without a path:

```toml
[instructions.team-style]
targets = ["codex", "claude-code"]
```

Selection precedence is CLI + model, CLI, model, then base. An unknown model
never matches a model selector. Every body is pinned, including variants not
selected on the current machine.

Instructions use the managed global or project instruction channel supported by
each CLI. They are not dynamically skill-loaded and do not belong inside a
toolset's `skills` list.
