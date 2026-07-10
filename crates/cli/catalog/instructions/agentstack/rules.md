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
