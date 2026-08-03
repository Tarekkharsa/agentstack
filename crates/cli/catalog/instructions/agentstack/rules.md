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

## Delivery is routed — read it before "fixing" a missing file

`agentstack delivery` prints, per tool, which kinds are served live and which
are written to files. Check it first.

- On an MCP-capable tool, **skills and MCP servers are served live** over that
  tool's gateway lease. There is nothing on disk to repair: a missing
  `.mcp.json` or `.claude/skills/` is expected — **do not create one**. Skills
  arrive through the `agentstack` MCP tools (`agentstack_list_loadable` to
  browse, `agentstack_load(name, reason)` for the body).
- **House rules, settings, hooks and extensions are always written to files**,
  and on a tool that reads files only, so is everything else.
- Live serving requires an open lease naming a toolset (the manifest's
  `profiles` table). Without one the gateway offers control-plane tools only —
  nothing is served implicitly. `agentstack lease status` shows what is open.
- The single override is `agentstack delivery render-locally --write` (add
  `--harness <id>` for one tool, `--off` to undo). It records
  `[delivery] render_locally`, so every clone answers the same way.

## Authoring: library first, dropping a file second

The library is one or more **linked folders**. `agentstack lib sources` shows
them in precedence order and names what shadows what; the first source holding
a name wins. Author there — `agentstack lib new <name>` scaffolds
`./<name>/SKILL.md`, `agentstack lib add <source> --write` takes a skill in,
`agentstack lib link <folder> --write` links another folder as a source.

Dropping a file into the project stays the quick capture. Write it and stop:

- a skill → `.agentstack/skills/<name>/SKILL.md`
- an instruction fragment → `.agentstack/instructions/<name>.md`

An undeclared file there is **inert** — nothing resolves, pins, renders, or
reads it, and it enters no agent's context. That is the point: you can create
it without asking, because creating it grants nothing.

Then tell the user to run **`agentstack yes`**. It shows one review — what will
be declared, what will be pinned, what will be written, and the full consent
surface — and their single confirmation makes it live. Do not run it for them:
it is the human's yes, and it refuses without a terminal anyway.

Do not hand-edit `[skills.*]` / `[instructions.*]` to declare what you just
wrote, and do not run `lock`/`trust`/`use` yourself to shortcut the review.
Content you did not write — anything that came with the repository — is not
eligible for that path and takes the full staged review
(`agentstack adopt` → `lock` → `trust`), which is also the path to name when
`yes` says a file was held back.

Servers are different: they carry commands, env, and secrets, so there is no
file to drop — declare them (`agentstack add server …`). Hooks and extensions
are executable: they are declared (`agentstack lib add-hook`,
`agentstack lib add-extension`), never dropped, and always get the full
ceremony.

## Running

`agentstack run <cli>` is **Protected by default** — trust, strict lock
verification, policy admission, a frozen grant. `--unprotected` opts out into a
plain host launch with none of those gates: an escape hatch, not the daily
path. Name the toolset with `--toolset <name>` (`--profile` is the older
spelling); an unfenced run contributes no package servers at all.

`agentstack workflow run <name>` drives a reviewed multi-agent task, each role
bound to a toolset that may set its own `model` and `effort`.
`agentstack image --toolset <name>` composes one toolset's pinned bytes into a
container image locally — nothing resolved into it, nothing pushed.

## Keep the loop closed

- After editing a toolset's `skills`/`servers`/`packages`, re-pin:
  `agentstack lock`, or `agentstack use <toolset> --write` which also renders.
  `doctor` treats lock drift as an error until you do. A selected package
  expands in the lock to its exact pinned members.
- Drift decision rule: a hand-added server you want to keep →
  `agentstack adopt --write` (pull it into the manifest); the manifest is the
  truth → `agentstack apply --write` (re-render). Never edit a rendered file
  to "fix" drift.
- After changing `[instructions.*]`, recompile: `agentstack instructions
  --write`. A fragment may carry per-(CLI, model) bodies under
  `[[instructions.<name>.variant]]`; the model is known only from an
  explicitly named toolset's `model` or `[settings.<cli>] model`, and an
  unknown model never matches a `model` selector.
- Verify with `agentstack doctor`; undo a bad write with `agentstack restore`
  or `agentstack undo`.
