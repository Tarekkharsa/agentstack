# GitHub Industry Research: Agent Config And MCP Managers

Date: 2026-06-25

## Research Goal

Find GitHub projects adjacent to agentstack and identify features agentstack should support, improve on, or intentionally skip.

This research focused on projects that manage:

- MCP server config across AI coding tools
- skills/instructions/rules across harnesses
- profiles or selective activation
- secrets and local overrides
- dashboard or GUI management
- live MCP health checks
- runtime/gateway behavior
- security, policy, or context-budget controls

## Executive Summary

There are already direct competitors for "one source of truth for MCP/skills/instructions." The strongest direct comparables are:

- [`amtiYo/agents`](https://github.com/amtiYo/agents)
- [`raintree-technology/agent-starter`](https://github.com/raintree-technology/agent-starter)
- [`Leoyang183/sync-agents-settings`](https://github.com/Leoyang183/sync-agents-settings)
- [`mcpware/cross-code-organizer`](https://github.com/mcpware/cross-code-organizer)
- [`tylergraydev/claude-code-tool-manager`](https://github.com/tylergraydev/claude-code-tool-manager)

There are also adjacent projects with important runtime ideas:

- [`openclaw/mcporter`](https://github.com/openclaw/mcporter)
- [`docker/mcp-gateway`](https://github.com/docker/mcp-gateway)
- [`mksglu/context-mode`](https://github.com/mksglu/context-mode)
- [`gs-init/ai-apparatus`](https://github.com/gs-init/ai-apparatus)
- [`jritsema/mcp-cli`](https://github.com/jritsema/mcp-cli)

The conclusion is clear:

agentstack should not compete only on "we sync MCP configs." That feature is already becoming table stakes.

agentstack should compete on:

- safer writes
- better secret handling
- adapter extensibility
- policy and governance
- lockfile-backed reproducibility
- explainability
- live health checks
- smart activation
- team bootstrap
- private catalogs/recipes
- optional integration with runtime/gateway tools instead of rebuilding them

## Snapshot Of Relevant Projects

| Project | Stars observed | Main angle | Agentstack implication |
|---|---:|---|---|
| [`openclaw/mcporter`](https://github.com/openclaw/mcporter) | 4697 | MCP runtime, discovery, typed calls, OAuth, record/replay, bridge mode | Do not rebuild full runtime first. Integrate/probe/call through compatible runtime ideas. |
| [`lastmile-ai/mcp-agent`](https://github.com/lastmile-ai/mcp-agent) | 8384 | Agent framework using MCP config + secrets split | Mostly adjacent; reinforces config/secrets split. |
| [`docker/mcp-gateway`](https://github.com/docker/mcp-gateway) | 1468 | Containerized MCP gateway, catalog, OAuth, secrets, profiles, tool allowlists | Strong signal that gateway + isolation + catalogs matter. |
| [`tylergraydev/claude-code-tool-manager`](https://github.com/tylergraydev/claude-code-tool-manager) | 345 | GUI for Claude Code MCP management, multi-editor sync | Dashboard UX and installed-editor detection matter. |
| [`mcpware/cross-code-organizer`](https://github.com/mcpware/cross-code-organizer) | 341 | Cross-harness dashboard, skills, memory, sessions, security scanning, backups | agentstack should own trust/policy/health if it wants to beat dashboards. |
| [`raintree-technology/agent-starter`](https://github.com/raintree-technology/agent-starter) | 81 | Project-local agent.json, skills, stack profiles, curated skill pack | Copy stack-profile and high-quality recipes idea. |
| [`jellydn/my-ai-tools`](https://github.com/jellydn/my-ai-tools) | 84 | Personal/team setup replication across many AI tools | Shows demand for complete setup bundles. |
| [`amtiYo/agents`](https://github.com/amtiYo/agents) | 74 | `.agents/` source of truth for MCP, skills, instructions across many tools | Direct competitor. Need parity on setup, watch, check, status, targets. |
| [`jritsema/mcp-cli`](https://github.com/jritsema/mcp-cli) | 12 | CLI for MCP config files, profiles, expanded commands | Copy simple profile UX, avoid secret-leaking command output by default. |
| [`Leoyang183/sync-agents-settings`](https://github.com/Leoyang183/sync-agents-settings) | 8 | Claude Code as source of truth, broad adapter support, plugin slash commands | Copy JSON reports, reconcile, backups, slash-command workflow. |
| [`0xRaghu/unified-mcp-manager`](https://github.com/0xRaghu/unified-mcp-manager) | 4 | Browser GUI, profiles, connection testing | Copy profile import/export and connection test UX if useful. |
| [`gs-init/ai-apparatus`](https://github.com/gs-init/ai-apparatus) | 0 | Fork-friendly skill/MCP marketplace with credentials file | Copy private catalog and credential-discovery patterns, but improve secret storage. |

Star counts came from the GitHub API during this review and will change.

## Direct Competitors

### 1. `amtiYo/agents`

Repository: <https://github.com/amtiYo/agents>

Why it matters:

This is the closest conceptual competitor. It offers one `.agents/` source of truth and syncs MCP servers, skills, and instructions across Codex, Claude Code, Claude Desktop, Gemini CLI, Cursor, Copilot, Antigravity, Windsurf, OpenCode, and Junie.

Notable features:

- Interactive `agents start` setup wizard.
- `agents sync` to materialize configs.
- `agents sync --check` for strict read-only drift checks.
- `agents watch` to auto-sync on `.agents/` changes.
- `agents status` for integrations, MCP servers, file states, and live probes.
- `agents doctor` and `agents doctor --fix`.
- `agents mcp test --runtime`.
- Secret splitting from public config to `.agents/local.json`.
- Secret-like value detection.
- Literal-secret warning in `doctor`.
- Git strategy that commits source config and skills while generated outputs can stay ignored.
- Broad target support, including Copilot CLI, Antigravity, Windsurf, OpenCode, Junie.
- Documentation injection via `agents start --inject-docs`.

What agentstack already covers well:

- Neutral manifest instead of source-tool sync.
- Rust single binary.
- Adapter descriptors.
- OS keychain/varlock/.env secret resolution chain.
- Lockfile/store concept for skills.
- Non-destructive merge tests.
- Dashboard.
- MCP server mode for agent-assisted manifest edits.

What agentstack should copy or beat:

- Add `watch`.
- Add interactive `start` or `bootstrap`.
- Add `sync/check` equivalent with stable exit codes.
- Add runtime MCP tests beyond handshake.
- Add docs injection or generated onboarding block.
- Add more targets: Claude Desktop, Copilot CLI, OpenCode, Junie, Antigravity.
- Add secret literal scanner.
- Add an explicit source/generated file strategy.

Priority:

High. This repo defines the current parity bar.

### 2. `agent-starter`

Repository: <https://github.com/raintree-technology/agent-starter>

Why it matters:

This project is not just config sync. It packages a strong project starter experience: an `agent.json` manifest, curated skills, stack profiles, drift status, MCP catalog additions, and generated native files for Claude Code, Codex, and Cursor.

Notable features:

- Project-local `agent.json`.
- Skills, MCP servers, and stack profiles.
- `sync` and `status`.
- Idempotent generated sections with manual edits preserved.
- Generated outputs for Claude Code, Codex, and Cursor.
- Stack profiles such as `next-saas`, `next`, `node`, and `base`.
- `init` auto-detects the right profile from `package.json`.
- Ships 29 hand-maintained skills.
- `finish-setup` skill can guide provisioning through wired MCPs.
- `.env.example` warning/appending for missing variables.
- Token-count benchmarks for representative workloads.

What agentstack should copy or beat:

- Stack-aware profiles.
- Curated recipe packs, not just individual MCP servers.
- High-quality bundled skills as examples.
- A `finish-setup` or `bootstrap` agent workflow.
- Missing-secret documentation generation.
- Benchmark or context-footprint reporting.

Priority:

High for product packaging. This project shows that curated workflows are more compelling than raw config management.

### 3. `sync-agents-settings`

Repository: <https://github.com/Leoyang183/sync-agents-settings>

Why it matters:

This is a narrower but mature sync utility. It uses Claude Code as source of truth and syncs MCP configs/instructions to many targets. It has strong CLI ergonomics around dry runs, validation, reconciliation, JSON reports, backup, and Claude plugin commands.

Notable features:

- Broad target list: Gemini CLI, Codex CLI, OpenCode, Kiro, Cursor, Kimi, Vibe, Qwen, Amp, Cline, Windsurf, Aider.
- Dry-run preview.
- Automatic timestamped backups.
- `list`, `diff`, `doctor`, `validate`, `reconcile`.
- `doctor --fix`.
- `reconcile --report json`.
- CI-friendly JSON reports for doctor, validate, sync, diff, instruction sync.
- Report schema generation/check.
- Slash commands as a Claude plugin: `/sync`, `/sync-list`, `/sync-diff`, `/sync-doctor`, `/sync-validate`, `/sync-reconcile`, `/sync-instructions`.
- Sync-awareness skill that notices edits to MCP settings or instructions and suggests syncing.
- Instruction sync with target-specific formats and import safety rules.
- OAuth-only server skip/manual setup warnings.
- Custom home dirs per target.

What agentstack should copy or beat:

- Stable JSON report output for CI and integrations.
- `reconcile` command.
- Timestamped backups, not just one rolling backup.
- Plugin/slash-command workflow for Claude Code.
- Sync-awareness skill for agentstack-managed files.
- More adapters.
- Explicit OAuth handling behavior.
- Instruction import safety controls.

Priority:

High for CLI maturity and CI friendliness.

### 4. `cross-code-organizer`

Repository: <https://github.com/mcpware/cross-code-organizer>

Why it matters:

This is a dashboard-heavy cross-harness organizer. It reaches beyond MCP config into sessions, memories, backups, security scanning, context budget, and config health.

Notable features:

- Harness selector for Claude Code and Codex CLI.
- Dashboard for settings, skills, MCP servers, profiles, sessions, history, runtime files, and backups.
- Security scanning language.
- Context budget features.
- Planned config health score.
- Planned cross-harness portability for Claude Code, Codex, Cursor, Windsurf, Aider.
- Planned CLI/JSON output for CI.
- Planned team config baselines.
- Cost tracker exploration.

What agentstack should copy or beat:

- Config health score.
- Backup browser/restore UX.
- Security scanning or at least policy risk scoring.
- Context budget / capability footprint.
- Team baselines.
- JSON output for headless scans.

Priority:

Medium-high. This is more of a dashboard competitor, but it points toward features users will expect.

### 5. `claude-code-tool-manager`

Repository: <https://github.com/tylergraydev/claude-code-tool-manager>

Why it matters:

This is a GUI app with multi-editor sync for global and project MCP configs. It supports Claude Code, Cursor, Gemini CLI, and more.

Notable features:

- GUI management.
- Multi-editor sync.
- Global MCPs sync to each editor's global config.
- Project MCPs sync to project-level configs such as `.cursor/mcp.json` and `.gemini/settings.json`.
- Installed editor detection via PATH or app bundles.

What agentstack should copy or beat:

- Better app-bundle detection on macOS, not only CLI binary detection.
- Clear global/project UI.
- One screen showing "where this server is active."

Priority:

Medium. agentstack already has a dashboard, but installed-client detection can improve.

## Adjacent Runtime And Gateway Projects

### 6. `mcporter`

Repository: <https://github.com/openclaw/mcporter>

Why it matters:

mcporter is not a config sync tool. It is a runtime, CLI, and TypeScript toolkit for discovering configured MCP servers, calling tools, generating typed clients, recording/replaying traffic, OAuth, ad-hoc connections, and bridge mode.

Notable features:

- Zero-config discovery from home config, project config, and imports from Cursor, Claude, Codex, Windsurf, OpenCode, VS Code.
- `list` with schema/status/JSON output.
- `call` to invoke MCP tools directly.
- Ad-hoc HTTP/stdio connections without editing config.
- Persist ad-hoc servers later.
- OAuth caching and headless OAuth.
- Typed TypeScript client generation.
- CLI generation from MCP server definitions.
- Record/replay MCP JSON-RPC fixtures.
- Bridge mode exposing multiple daemon-managed servers through one MCP bridge.
- Tool filtering with `allowedTools` and `blockedTools`.
- Atomic serialized writes.
- Issue discussion around on-demand activation for heavy MCP servers to reduce context overhead.

What agentstack should copy or integrate:

- Tool allow/block lists in manifest.
- `agentstack probe` / `doctor --probe` with tool counts/resources/prompts.
- Optional `agentstack call <server.tool>` for diagnostics.
- Ad-hoc "try this MCP" flow before persisting.
- OAuth metadata and auth lifecycle.
- Record/replay for support bundles.
- On-demand activation for heavy MCP servers.

Important strategic point:

agentstack should not become a full MCP runtime too early. It can integrate with mcporter-style runtimes or support exporting to them.

Priority:

High for runtime-adjacent ideas, but not necessarily for immediate implementation.

### 7. Docker MCP Gateway

Repository: <https://github.com/docker/mcp-gateway>

Why it matters:

Docker is solving MCP server lifecycle, isolation, catalogs, secrets, OAuth, profiles, and gateway consistency. This is a serious industry signal.

Notable features:

- Container-based MCP servers.
- Unified gateway that clients connect to.
- Docker Desktop secrets management.
- OAuth integration.
- Server catalog management.
- Dynamic tool/prompt/resource discovery.
- Logging and call tracing.
- Profiles grouping MCP servers.
- Profile export/import.
- Push/pull profiles to OCI registries.
- Tool enable/disable allowlists at profile level.
- Client connect commands.

What agentstack should copy or integrate:

- Treat gateway profiles as a target.
- Add Docker MCP Gateway adapter/export.
- Add profile export/import.
- Add tool-level allowlists.
- Track server isolation mode: native stdio, remote HTTP, docker gateway.
- Support private catalogs.

Priority:

High for enterprise/devex positioning.

### 8. `context-mode`

Repository: <https://github.com/mksglu/context-mode>

Why it matters:

This project is about context-window optimization, hooks, routing, session continuity, and security enforcement across many agent platforms. It demonstrates that hooks are becoming as important as MCP config.

Notable features:

- MCP + hooks across many platforms.
- PreToolUse/PostToolUse/SessionStart/PreCompact support matrix.
- Routing instructions plus hook enforcement.
- Context budget and savings metrics.
- Session continuity through hooks.
- `ctx doctor`, `ctx stats`, `ctx upgrade`.
- Security policy reuse from Claude settings style.
- Blocks dangerous commands and path traversal in sandbox tools.
- Redacts tool inputs before persistence.

What agentstack should copy or integrate:

- Manage hooks as first-class capabilities.
- Add hook support to adapter descriptors.
- Add context-footprint/capability-footprint metrics.
- Add capability risk scoring for hookable/blockable tools.
- Add "minimal context mode" profiles.
- Add platform support matrix for hooks.

Priority:

Medium-high. Hooks are likely the next layer after MCP/skills/settings.

### 9. `ai-apparatus`

Repository: <https://github.com/gs-init/ai-apparatus>

Why it matters:

This is an open, fork-friendly marketplace of skills and optional MCP servers for platform/application teams.

Notable features:

- Presets for Cursor, Claude Desktop, Claude Code, VS Code/Copilot, generic.
- Skills and MCP servers organized under marketplace directories.
- Symlinked skills from repo to agent skill dirs.
- Cursor session hook can git pull and auto-sync skills.
- Opt-in MCP install.
- Credentials file at `~/.config/ai-apparatus/credentials.yml`.
- AWS profile auto-discovery.
- `mcp-list`, `mcp-install`, `mcp-install --all`.

What agentstack should copy or beat:

- Private/forkable catalogs.
- Session-start update hooks, but safer and explicit.
- Cloud credential/profile discovery.
- Marketplace layout for internal teams.
- Better secret storage than plaintext credentials YAML.

Priority:

Medium.

### 10. `mcp-cli`

Repository: <https://github.com/jritsema/mcp-cli>

Why it matters:

Small focused CLI for MCP config pain. It explicitly names profiles, multiple config files, secret envvars, experimentation, and switching configurations as pain points.

Notable features:

- Manages MCP config files.
- Profiles for different work modes.
- Command output for agents/scripts with envvars expanded.
- Simple list/get/add style ergonomics.

What agentstack should copy or avoid:

- Copy the simple profile UX.
- Add "copy command" only behind an explicit unsafe flag because expanded envvars can leak secrets.

Priority:

Low-medium.

## Feature Parity Checklist

### Must Have

These should be implemented or clearly planned because competitors already set user expectations:

- `agentstack bootstrap` or `agentstack start` interactive setup.
- `agentstack watch`.
- `agentstack status` with files, targets, active capabilities, and health.
- `agentstack sync --check` or equivalent strict read-only drift check with stable exit code.
- `agentstack reconcile` for "fix missing/out-of-sync managed entries only."
- `agentstack explain <capability>`.
- `agentstack report --json` or JSON output for `doctor`, `diff`, `apply`, `status`.
- Timestamped backups and backup listing, not only rolling backup.
- Secret literal scanner.
- Runtime MCP probe/test beyond initialize handshake.
- More adapters: Claude Desktop, Copilot CLI, Copilot VS Code, OpenCode, Kiro, Qwen, Kimi, Amp, Cline, Antigravity, Junie.
- Tool-level allow/block lists.
- OAuth-aware server states.

### Should Have

These are not table stakes yet, but they would make agentstack better:

- Stack-aware profile detection.
- Recipe bundles for workflows.
- Private catalogs.
- Profile export/import.
- Docs/onboarding block injection.
- `.env.example` or secret setup guide generation.
- Capability diff for PR review.
- Config health score.
- Context/capability footprint score.
- Hook management in adapter descriptors.
- Gateway target/export for Docker MCP Gateway and/or mcporter.
- On-demand activation for heavy MCP servers.

### Nice To Have

- Agentstack Claude plugin/slash commands.
- Sync-awareness skill.
- Record/replay support bundles.
- Typed tool client generation by delegating to mcporter-style tooling.
- Cost tracker.
- Session/history viewer.
- Credential provider discovery for AWS/GCP/Azure.

## Where Agentstack Can Be Better

### 1. Safer secret model

Competitors commonly use gitignored JSON/YAML local files. agentstack already has a stronger direction with env, varlock, OS keychain, and `.env` fallback.

Action:

- Keep keychain/varlock as a differentiator.
- Add secret literal scanning.
- Block unresolved secrets before writes.
- Generate missing-secret setup docs.

### 2. Better adapter architecture

Most competitors hardcode tool targets in TypeScript/JavaScript. agentstack's YAML descriptors are a strong advantage.

Action:

- Document the adapter descriptor format.
- Add conformance tests per adapter.
- Let users run `agentstack adapters validate`.
- Encourage community adapters.

### 3. Trustable writes

Many tools say "preserve manual edits." agentstack has strong test coverage here.

Action:

- Add timestamped backup history.
- Add `agentstack restore list`.
- Add `agentstack history`.
- Add write-plan IDs for dashboard writes.

### 4. Governance

Competitors mention security, but few appear to offer a full policy layer for sources, secrets, local execution, pinning, and tool allowlists.

Action:

- Make `[policy]` a major feature.
- Add risk scoring.
- Add CI JSON reports.
- Add PR capability diffs.

### 5. Reproducibility

agentstack's lockfile can become a major differentiator if hardened.

Action:

- Use SHA-256 lock checksums.
- Pin git skills by commit.
- Record catalog/provider provenance.
- Support `install --locked` as a team/CI story.

## What Not To Copy Blindly

### Do not become Claude-source-of-truth only

`sync-agents-settings` does this well, but agentstack's neutral manifest is more durable.

### Do not rely only on plaintext local secret files

Gitignored files are useful, but keychain/varlock support is more professional.

### Do not become a full MCP runtime too early

mcporter and Docker MCP Gateway are already strong. agentstack should manage/export/integrate first.

### Do not chase every adapter before the trust flow is excellent

Broad adapter support helps, but users will abandon the tool if one write breaks their config.

### Do not make the dashboard the only good UX

The CLI must stay excellent for CI, automation, SSH, and agents.

## Recommended Agentstack Roadmap From Research

### Phase 1: Parity And Trust

1. Block unresolved secrets and structural validation errors before writes.
2. Add `status`.
3. Add `explain`.
4. Add JSON output for `doctor`, `diff`, `apply`, and `status`.
5. Add timestamped backup history.
6. Add secret literal scanner.
7. Add `sync --check` or `doctor --ci --strict` with stable exit codes.

### Phase 2: Competitive Workflow

1. Add `bootstrap` interactive setup.
2. Add `watch`.
3. Add `reconcile`.
4. Add runtime MCP probe/test.
5. Add stack-aware profile detection.
6. Add `.env.example` or missing-secret setup guide generation.

### Phase 3: Differentiation

1. Add capability risk scoring.
2. Add policy rules for local execution, pinning, sources, and tool allowlists.
3. Add PR capability diff.
4. Add recipes.
5. Add private catalogs.
6. Add adapter descriptor docs and conformance suite.

### Phase 4: Runtime/Gateway Integration

1. Add Docker MCP Gateway export/adapter.
2. Add mcporter export/probe/call integration.
3. Add OAuth lifecycle metadata.
4. Add on-demand activation for heavy MCP servers.
5. Add record/replay support bundles if users need debugging.

## Strongest Product Wedge After Research

The market already has sync tools. The sharper wedge is:

> The safest way for teams to declare, review, activate, and verify agent capabilities across every AI coding harness.

That means agentstack should lead with:

- reviewable manifest
- no leaked secrets
- policy gate
- reproducible lockfile
- safe diffs and backups
- active health checks
- explainability
- smart profiles

The direct competitors prove the need exists. agentstack can win if it is more trustworthy and more governable than the Node-based sync tools, while still matching their convenience.

## Source Links

- `amtiYo/agents`: <https://github.com/amtiYo/agents>
- `raintree-technology/agent-starter`: <https://github.com/raintree-technology/agent-starter>
- `Leoyang183/sync-agents-settings`: <https://github.com/Leoyang183/sync-agents-settings>
- `0xRaghu/unified-mcp-manager`: <https://github.com/0xRaghu/unified-mcp-manager>
- `tylergraydev/claude-code-tool-manager`: <https://github.com/tylergraydev/claude-code-tool-manager>
- `mcpware/cross-code-organizer`: <https://github.com/mcpware/cross-code-organizer>
- `jritsema/mcp-cli`: <https://github.com/jritsema/mcp-cli>
- `openclaw/mcporter`: <https://github.com/openclaw/mcporter>
- `docker/mcp-gateway`: <https://github.com/docker/mcp-gateway>
- `gs-init/ai-apparatus`: <https://github.com/gs-init/ai-apparatus>
- `mksglu/context-mode`: <https://github.com/mksglu/context-mode>
- `lastmile-ai/mcp-agent`: <https://github.com/lastmile-ai/mcp-agent>
- `jellydn/my-ai-tools`: <https://github.com/jellydn/my-ai-tools>

