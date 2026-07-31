# agentstack house rules

How to behave around agentstack-managed setups (a manifest at
`.agentstack/agentstack.toml`, legacy root `agentstack.toml`, or the personal
layer `~/.agentstack/agentstack.toml`).

## The model

- The manifest is the source of truth; native CLI configs (`.mcp.json`,
  `~/.claude.json`, Codex `config.toml`, `.claude/skills/`, managed regions of
  `CLAUDE.md`/`AGENTS.md`) are compiled output. Change the manifest, never the
  output.
- Secrets are `${REF}` placeholders resolved per machine. Never write a secret
  value into a manifest, library, or config — tell the user to run
  `agentstack secret set REF`. A blocked write ("unresolved secret") is a
  feature: surface which `${REF}` is missing, don't work around it.
- Nothing touches disk without `--write`; dry-run output is always safe.
  Propose (edit the manifest, show the dry-run), let a human apply.

## Authoring a skill or instruction: write the file, let the user say yes

Dropping a file into the project is a first-class way to author a capability.
Write it where it belongs and stop there:

- a skill → `.agentstack/skills/<name>/SKILL.md`
- an instruction fragment → `.agentstack/instructions/<name>.md`

An undeclared file there is **inert** — nothing resolves, pins, renders, or
reads it, and it enters no agent's context. That is the point: you can create
it without asking, because creating it grants nothing.

Then tell the user to run **`agentstack yes`**. It shows one review — what will
be declared, what will be pinned, what will be written, and the full consent
surface — and their single confirmation makes it live in every CLI. Do not run
it for them: it is the human's yes, and it refuses without a terminal anyway.

Do not hand-edit `[skills.*]` / `[instructions.*]` to declare what you just
wrote, and do not run `lock`/`trust`/`use` yourself to shortcut the review.
Content you did not write — anything that came with the repository — is not
eligible for that path and takes the full staged review
(`agentstack adopt` → `lock` → `trust`), which is also the path to name when
`yes` says a file was held back.

Servers are different: they carry commands, env, and secrets, so there is no
file to drop — declare them (`agentstack add server …`). Hooks and extensions
are executable and always get the full ceremony.

## Recognize the artifact mode before "fixing" anything

1. **Static** — `.mcp.json` / `.claude/skills/` exist on disk, gitignored via a
   managed block. Activate with `agentstack use <profile> --scope project --write`.
2. **Clean-at-rest** — nothing generated exists between sessions; capabilities
   appear only during `agentstack run <cli> --profile <p>` or between
   `agentstack session start <p>` and `session end`. A missing `.mcp.json`
   here is **intentional — do not create one**, and do not hand-create
   `.claude/skills/`.
3. **Zero-files / MCP** — skills arrive through the `agentstack` MCP tools
   (`agentstack_list_loadable` to browse, `agentstack_load(name, reason)` for
   the body); there is nothing on disk to repair.

## Keep the loop closed

- After editing a profile's `skills`/`servers` lists, re-lock:
  `agentstack use <profile> --write` refreshes `agentstack.lock`, and
  `doctor` treats lock drift as an error until you do.
- Drift decision rule: a hand-added server you want to keep →
  `agentstack adopt --write` (pull it into the manifest); the manifest is the
  truth → `agentstack apply --write` (re-render). Never edit the rendered
  file to "fix" drift.
- After changing `[instructions.*]`, recompile: `agentstack instructions --write`.
- Verify with `agentstack doctor`; undo a bad write with `agentstack restore`.
