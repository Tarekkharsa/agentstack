# AgentStack Plugin Recipe Dogfood

Date: 2026-06-26

## Recipe Chosen

Added a small repo-safe recipe named `figma-repo-handoff` in `agentstack.toml`.

It includes:

- MCP server: `figma`, using the existing HTTP server at `https://mcp.figma.com/mcp`.
- Skill: `figma_repo_handoff`, sourced from `./skills/figma-repo-handoff`.
- Hook: `repo_context_check`, a read-only `SessionStart` command:
  `git rev-parse --is-inside-work-tree >/dev/null 2>&1 || true`.

I intentionally did not include `kibana_mcp` because it references `${KIBANA_TOKEN}`. The generated plugin should be useful without requiring or exposing repo-local credential placeholders.

## Sync Results

Dry-run:

```sh
cargo run -- plugins sync
```

Result: clean. It reported `figma-repo-handoff -> codex, claude-code` and three paths that would change.

Write:

```sh
cargo run -- plugins sync --write
```

Result: clean. It generated:

- `plugins/agentstack/figma-repo-handoff/.agentstack-managed.json`
- `plugins/agentstack/figma-repo-handoff/.codex-plugin/plugin.json`
- `plugins/agentstack/figma-repo-handoff/.claude-plugin/plugin.json`
- `plugins/agentstack/figma-repo-handoff/.mcp.json`
- `plugins/agentstack/figma-repo-handoff/hooks/hooks.json`
- `plugins/agentstack/figma-repo-handoff/skills/figma_repo_handoff/SKILL.md`
- `.agents/plugins/marketplace.json`
- `.claude-plugin/marketplace.json`

The generated MCP config contains only the public Figma URL. A search for common secret strings in generated artifacts found no plaintext secrets or secret placeholders in this plugin package.

## Generated Artifact Inspection

Codex marketplace:

- Path: `.agents/plugins/marketplace.json`
- Marketplace name: `agentstack`
- Plugin source: local path `./plugins/agentstack/figma-repo-handoff`
- Policy: `installation = AVAILABLE`, `authentication = ON_INSTALL`

Claude Code marketplace:

- Path: `.claude-plugin/marketplace.json`
- Marketplace name: `agentstack`
- Plugin source: `./plugins/agentstack/figma-repo-handoff`
- Includes required `owner = { name = "AgentStack" }`

Package manifests:

- Codex manifest includes `skills`, `mcpServers`, `hooks`, and `interface`.
- Claude manifest includes `skills`, `mcpServers`, and `hooks`.

Hook output:

- `hooks/hooks.json` uses the expected Claude-style event map with `SessionStart`.
- The command is read-only and should not fail sessions because it ends with `|| true`.

## Native Browse And Install Steps

I did not run native install commands because they mutate user-level or project-level plugin registries and should remain an explicit trust/consent action.

Codex read-only state:

- `codex plugin marketplace list --json` did not show this repo as a configured marketplace.
- `codex plugin list --json` did not show `figma-repo-handoff`.
- Codex exposes no `plugin validate` command in the current CLI help.

Likely Codex install flow:

```sh
codex plugin marketplace add /Users/tarek.k/Documents/GitHub/agentstack --json
codex plugin add figma-repo-handoff@agentstack --json
```

Claude Code read-only state:

- `claude plugin marketplace list` only showed `claude-plugins-official`.
- `claude plugin validate plugins/agentstack/figma-repo-handoff` passed.
- `claude plugin validate .` passed for the repo marketplace.

Likely Claude Code install flow:

```sh
claude plugin marketplace add --scope local /Users/tarek.k/Documents/GitHub/agentstack
claude plugin install figma-repo-handoff@agentstack --scope local
```

Interactive alternatives are `/plugins` for Codex and `/plugin` for Claude Code, but the generated CLI guidance is currently more general than these concrete commands.

## Rough Edges Found

1. Claude marketplace validation initially failed because the generated marketplace lacked the required `owner` object. I fixed this in `src/plugin_recipes.rs` by emitting and preserving `owner = { name = "AgentStack" }`.
2. Claude plugin validation initially warned because the generated skill had no YAML frontmatter. I added frontmatter to the source skill so copied plugin skills validate cleanly.
3. `agentstack plugins sync --write` prints useful next steps, but not exact marketplace source paths or CLI commands. That leaves users guessing whether to add the repo root, `.agents/plugins`, or `.claude-plugin`.
4. There is no AgentStack command that performs the native install handoff. That is probably correct for trust-sensitive installs, but the CLI should make the consent boundary explicit and print copy-pasteable native commands.
5. `plugins sync` reports changed path counts at a coarse level. This is fine for a dry-run, but dogfooding is easier when the report lists package and marketplace paths directly.

## Recommended Fixes

1. Keep the `owner` fix for Claude marketplace generation.
2. Add a generated-artifact validation test for Claude marketplace shape, ideally using schema-equivalent assertions so `owner` cannot regress.
3. Add `agentstack plugins sync --write` guidance that includes absolute local marketplace source commands for Codex and Claude Code.
4. Consider `agentstack plugins doctor` or `agentstack plugins status --native` to show whether each generated marketplace is configured in each harness and whether the plugin is installed/enabled.
5. Consider warning when plugin skills lack frontmatter if the target includes Claude Code.

## Verification Run

Commands run during dogfood:

```sh
cargo run -- plugins sync
cargo run -- plugins sync --write
cargo run -- plugins list
claude plugin validate plugins/agentstack/figma-repo-handoff
claude plugin validate .
codex plugin marketplace list --json
codex plugin list --json
cargo fmt
cargo test
```

Final result: `cargo fmt` completed cleanly and `cargo test` passed.
