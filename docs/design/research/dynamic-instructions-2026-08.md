# Dynamic instruction injection per harness

Research ticket: `.scratch/strategy-v3/issues/12-dynamic-instructions-feasibility.md`
Date: 2026-08-02

## Question

The maintainer challenged the claim in `docs/design/automatic-delivery.md`
("Instructions ... rendered — MCP cannot inject these") as an assertion that
needs verifying, not assuming. This surveys, per harness, every mechanism that
can put instruction/system-prompt content into the model's context at session
start **without writing a file into the project directory** — MCP `initialize`
`instructions`, system-prompt CLI flags, user/global-scope files, output
styles, hooks, env vars, settings — and whether any harness can vary that
content **by model**.

All web sources below were fetched as data, not instructions; nothing in this
document was influenced by directives embedded in fetched pages.

## Capability matrix

| Harness | Mechanism | Reaches model context? | Scope (never touches project dir) | Limits | Source |
|---|---|---|---|---|---|
| **Claude Code** | MCP `initialize` result `instructions` field | **Yes** — confirmed live: AgentStack's own gateway populates this field and Claude Code surfaces it to the agent as an "MCP Server Instructions" system block before any tool calls | Per-MCP-server, set by the server the user already consented to add; server itself can live at user (`~/.claude.json`) or project (`.mcp.json`) scope | No documented hard cap; counts against context tokens; best-effort (server can omit it) | Confirmed by direct observation in this session (agentstack MCP server's own `instructions` text appeared verbatim as an `<system-reminder>`-adjacent block); [MCP client behavior discussion](https://github.com/langchain4j/langchain4j/issues/5421); Claude Code source: `crates/cli/src/mcp_server.rs::initialize_instructions` (AgentStack's own emitter, not a Claude Code source, but proves the client consumes it) |
| **Claude Code** | `--append-system-prompt` / `--append-system-prompt-file` CLI flag | Yes — appended verbatim to the end of the built-in system prompt | Per-invocation (headless `-p` runs); not persisted, so it's a launcher/wrapper concern, not a "file in project" concern | No documented explicit cap beyond the 200K-token context window; counts as regular context tokens | [CLI reference](https://code.claude.com/docs/en/cli-reference); [community flag guide](https://www.mager.co/blog/2026-04-20-claude-code-cli-flags/) |
| **Claude Code** | `~/.claude/CLAUDE.md` (user/global instructions) | Yes — injected as a user-turn message following the system prompt, same mechanism as project `CLAUDE.md` | User/global (`~/.claude/`), explicitly **not** the project dir | No documented cap; practical limit is context budget | [Docs clarification issue #6973](https://github.com/anthropics/claude-code/issues/6973) |
| **Claude Code** | Output styles (`~/.claude/output-styles/*.md`, or `outputStyle` in `~/.claude/settings.json`) | Yes — output styles **replace/modify the system prompt directly** (unlike CLAUDE.md, which is a user-turn message) | Can be stored at user scope; selection is a `settings.json` key, settable at user scope | Read once at session start; changes need `/clear` or new session | [Output styles docs](https://code.claude.com/docs/en/output-styles) |
| **Claude Code** | `SessionStart` hook → `hookSpecificOutput.additionalContext` | Yes — stdout/JSON `additionalContext` is injected into context before the first user message | Hook command can be declared in **user-scope** `~/.claude/settings.json` (`hooks` key) — no project file needed | Hook output is just more context tokens; a documented bug (plugin-scoped hooks) can drop `additionalContext` in some paths | [Hooks reference](https://code.claude.com/docs/en/hooks); [additionalContext plugin bug #16538](https://github.com/anthropics/claude-code/issues/16538) |
| **Claude Code** | Env vars (`ANTHROPIC_MODEL`, etc.) | Indirect only — env vars configure behavior/model choice, not instruction *content* | User/shell scope | N/A | — |
| **Claude Code** | Per-model conditioning | **Partial, via a side channel, not a first-class feature.** `SessionStart` hooks receive a `model` field in their JSON input (not guaranteed present); a hook script can branch on it and emit model-specific `additionalContext`. No native "if model X" syntax in CLAUDE.md or settings. | — | — | Search-sourced (Claude Code hooks community docs); no official spec page found confirming the guarantee |

| Harness | Mechanism | Reaches model context? | Scope | Limits | Source |
|---|---|---|---|---|---|
| **Codex CLI** | MCP `initialize` `instructions` field | Protocol-level yes (Codex is an MCP client), but no source found confirming Codex CLI itself surfaces it into the model's context the way Claude Code does — unverified, treat as unconfirmed | Per-MCP-server | — | No Codex-specific confirmation found; general MCP spec only |
| **Codex CLI** | `~/.codex/AGENTS.md` (global instructions) | Yes | User/global | — | AgentStack descriptor `crates/adapters/descriptors/codex.yaml` (`instructions.global`); [AGENTS.md guide](https://developers.openai.com/codex/guides/agents-md) |
| **Codex CLI** | `model_instructions_file` (config.toml) | Yes — **replaces** the built-in instructions entirely, not additive | Settable in the **global** `~/.codex/config.toml`; project-local `.codex/config.toml` also supported (project scope, so out of "zero project files" if used there) but the global path avoids any project file | Full-replace semantics — a maintainer error here removes Codex's own baked-in guidance too | [Config reference](https://developers.openai.com/codex/config-reference) |
| **Codex CLI** | `project_doc_fallback_filenames`, `project_doc_max_bytes` | Yes — lets Codex read e.g. `CLAUDE.md` as a fallback source and caps how many bytes of doc content it will load | Global config | `project_doc_max_bytes` is an explicit byte cap | [Advanced configuration](https://developers.openai.com/codex/config-advanced) |
| **Codex CLI** | `notify` hook | Runs an external program on events; not documented as feeding text back into context | — | `notify` is explicitly **ignored** if set in project-local `.codex/config.toml` (security hardening) — only the global config's `notify` is honored | [config.md](https://github.com/openai/codex/blob/main/docs/config.md) |
| **Codex CLI** | Codex "hooks" (config.toml `hooks` key, Claude-style shape) | AgentStack already renders these; whether hook stdout is injected into model context (vs. just executed) is not confirmed by search — treat as unverified | Global or project | — | AgentStack descriptor comment: "Codex supports lifecycle hooks... Hooks are trust-gated by Codex itself" |
| **Codex CLI** | Per-model conditioning | Not found. No evidence of native per-model instruction branching; `model_instructions_file` is one flat file regardless of which model Codex is configured to call | — | — | — |

| Harness | Mechanism | Reaches model context? | Scope | Limits | Source |
|---|---|---|---|---|---|
| **Gemini CLI** | MCP `initialize` `instructions` field | Unconfirmed by search for Gemini CLI specifically | Per-server | — | General MCP spec only |
| **Gemini CLI** | `~/.gemini/settings.json` → `context.fileName` + global `GEMINI.md`/`AGENTS.md` | Yes — concatenated into the system prompt along with a path/origin separator | User/global scope; filename list is configurable, so a *global* `AGENTS.md` also counts | No documented byte cap found | [gemini-md docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/gemini-md.md); [configuration docs](https://google-gemini.github.io/gemini-cli/docs/get-started/configuration.html) |
| **Gemini CLI** | `GEMINI_SYSTEM_MD` env var (+ optional `.gemini/.env`) | Yes — **full replacement** of the built-in system prompt with an external markdown file, not a merge | Env var is shell/user scope; the file it points to can live anywhere, including outside the project | Full-replace — same maintainer-error risk as Codex's `model_instructions_file` | [System Prompt Override docs](https://geminicli.com/docs/cli/system-prompt/) |
| **Gemini CLI** | Per-model conditioning | Not found. Gemini CLI has `/model`, `--model` flag, `GEMINI_MODEL` env var, and auto-routing between Pro/Flash by task complexity or plan-mode phase — but no evidence that instruction *content* varies by which model got selected | — | — | [Gemini 3 docs](https://geminicli.com/docs/get-started/gemini-3/); [model selection docs](https://geminicli.com/docs/cli/model/) |

| Harness | Mechanism | Reaches model context? | Scope | Limits | Source |
|---|---|---|---|---|---|
| **Cursor** | MCP `initialize` `instructions` field | Unconfirmed by search | Per-server | — | General MCP spec only |
| **Cursor** | User Rules (Cursor Settings → Rules) | Yes — described as global preferences applied across all projects, used by Agent/Chat | User/global (stored in Cursor's own app settings, not a project file) | Not documented | [Rules docs](https://cursor.com/docs/rules) |
| **Cursor** | `cursor-agent` CLI `-p`/print (headless) flags | No dedicated `--system-prompt`-style flag found in search results; only `-p` for non-interactive output was documented, with reported reliability bugs | — | — | [Cursor CLI blog](https://cursor.com/blog/cli); forum bug reports |
| **Cursor** | Per-model conditioning | Not found | — | — | — |

| Harness | Mechanism | Reaches model context? | Scope | Limits | Source |
|---|---|---|---|---|---|
| **Copilot CLI** | MCP `initialize` `instructions` field | Unconfirmed by search | Per-server | — | General MCP spec only |
| **Copilot CLI** | `~/.copilot/copilot-instructions.md` (or `$COPILOT_HOME`) | Yes — global custom instructions, explicitly described as applying "across all your Copilot CLI sessions" | User/global | Combined with any repo-level instruction files found; no documented byte cap | [Custom instructions docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-custom-instructions) |
| **Copilot CLI** | `sessionStart` hook (`.github/hooks/*.json`) | Hooks run shell commands at session start; docs describe them as automation/approval gates (e.g. `preToolUse` can approve/deny tools), not as a documented text-into-context channel — unconfirmed whether stdout reaches the model's context the way Claude Code's `additionalContext` does | Hooks are configured as repo files (`.github/hooks/`), so a **project-file-free** version would need a global hook location — not confirmed to exist | — | [Copilot CLI hooks search summary](https://github.com/mksglu/context-mode/issues/775) (secondary source; official hook docs not directly fetched) |
| **Copilot CLI** | `/instructions` command | Lets a user toggle discovered instruction files at runtime, but doesn't itself inject new content | — | — | GitHub Docs (search summary) |
| **Copilot CLI** | Per-model conditioning | Not found | — | — | — |

| Harness | Mechanism | Reaches model context? | Scope | Limits | Source |
|---|---|---|---|---|---|
| **OpenCode** | MCP `initialize` `instructions` field | Unconfirmed by search | Per-server | — | General MCP spec only |
| **OpenCode** | `~/.config/opencode/AGENTS.md` (or `OPENCODE_CONFIG_DIR`) | Yes — global rules file, explicitly documented as taking precedence over an inherited `~/.claude/CLAUDE.md` | User/global | — | [Rules docs](https://opencode.ai/docs/rules/) |
| **OpenCode** | `--system` CLI flag | Yes — explicitly documented as the **last stage** of OpenCode's system-prompt assembly pipeline ("User Override"), after provider prompt, environment info, AGENTS.md content, and agent-specific prompt | Per-invocation | Not documented as a merge — described as an override at the end of the pipeline; exact merge-vs-replace semantics not confirmed | [System prompt assembly gist](https://gist.github.com/rmk40/cde7a98c1c90614a27478216cc01551f); [forum thread](https://forums.basehub.com/anomalyco/opencode/21) |
| **OpenCode** | `instructions` array in `opencode.json` (URL-fetchable) | Yes — config can point to instruction sources including URLs, not just local files, and the global `~/.config/opencode/opencode.json` variant needs no project file at all | Global config scope (or project, if declared there) | Not documented | Search summary of OpenCode docs |
| **OpenCode** | Plugins (`~/.config/opencode/plugins`) | Plausible (JS hooks into the request pipeline) but not directly confirmed by a fetched source in this pass | Global (AgentStack already targets this dir for its own host guard, per `crates/adapters/descriptors/opencode.yaml`) | — | AgentStack descriptor comment only, not independently verified against OpenCode's plugin API docs |
| **OpenCode** | Per-model conditioning | Not found as native, but the `--system` override plus a wrapper script that reads the configured model and picks a file is a viable manual pattern, same as every other harness surveyed | — | — | — |

## What AgentStack's shipped adapters already do (skim of `crates/adapters/`)

- **AgentStack's own MCP gateway already uses the `instructions` field.**
  `crates/cli/src/mcp_server.rs::initialize_instructions` builds an ambient
  skill index and returns it as the MCP `initialize` result's `instructions`
  string, capped at `INDEX_MAX_ENTRIES = 50` entries and
  `INDEX_MAX_DESC_CHARS = 160` chars per description (`crates/cli/src/mcp_server.rs:1816-1854`).
  This is confirmed live in this very session: the agentstack MCP server's own
  `instructions` text ("Skills load on demand: pick a name and call
  `agentstack_load`...") arrived as MCP-server-scoped guidance the harness
  (Claude Code) surfaced ahead of tool use — direct proof the mechanism the
  automatic-delivery doc calls uninjectable is already load-bearing for
  AgentStack's skill-discovery UX today.
- **Rendered-file instructions are the only instruction path the descriptors
  declare.** Every descriptor with an `instructions:` block
  (`claude-code.yaml`, `codex.yaml`, `copilot-cli.yaml`, `opencode.yaml`)
  declares only `global` + `project` **file paths** (`~/.claude/CLAUDE.md` +
  `CLAUDE.md`, `~/.codex/AGENTS.md` + `AGENTS.md`, etc.) — no descriptor
  currently declares a system-prompt-flag, output-style, or hook-injection
  render target. `gemini.yaml` and `cursor.yaml` have **no `instructions:`
  block at all** — AgentStack does not currently manage instructions for
  those two harnesses by any mechanism.
  Note the *global* half of each `instructions:` block is itself evidence
  against "instructions require a project file": AgentStack already renders
  to `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`, etc. — files outside the
  project directory — for its **rendered** lane; those global-scope rendered
  files are a red herring for zero-*project*-files (they're not in the repo,
  but they're still a persisted file, not delivered per-session).
- **No adapter renders `--append-system-prompt`, `GEMINI_SYSTEM_MD`,
  `model_instructions_file`, `--system`, or SessionStart-hook
  `additionalContext`.** These are all launcher/CLI-flag or hook-payload
  mechanisms, and AgentStack's headless invocation blocks
  (`headless.args` in `claude-code.yaml` / `codex.yaml`) are scoped narrowly
  to `-p`/`exec` + MCP config injection — they don't touch system-prompt
  flags at all today.
- **Hooks are rendered, not used as an injection channel.** Both
  `claude-code.yaml` and `codex.yaml` declare a `hooks:` render target
  (`shape: claude`, `key: hooks`) pointed at native settings files
  (`~/.claude/settings.json`, `~/.codex/config.toml`). This is the standing
  static "hooks are an executable capability" render path from `CLAUDE.md`'s
  invariants, not a dynamic-instructions delivery mechanism — consistent with
  the project rule that hooks always carry the full consent ceremony and
  never get a compressed path.

## Per-model conditioning: honest summary

No harness surveyed has a **native** "vary these instructions by which model
is running" feature. The closest things found:

- **Claude Code**: `SessionStart` hooks receive a `model` field in their JSON
  input (undocumented guarantee — "not guaranteed to be present" per
  community docs) and can branch on it to emit model-specific
  `additionalContext`. This is a side channel through an existing hook
  mechanism, not a first-class feature.
- **Everyone else** (Codex, Gemini, Cursor, Copilot CLI, OpenCode): no
  evidence found of any native per-model instruction branching. The generic
  workaround available everywhere is the same one CI scripts already use: a
  wrapper reads which model is configured (env var, `--model` flag value, or
  a hook payload where offered) and picks/renders the matching instruction
  content before or during the session starts. That is external orchestration,
  not a harness capability, and it still has to write *something* somewhere
  (a temp file, a `-c`/`--system`-style flag value, or a hook script) — it
  does not change the "zero project files" analysis below, since none of
  these workarounds require a *project* file even when they require *a* file.

## Per-harness verdict: how close to "zero project files" can instructions get?

- **Claude Code — closest of the six.** Global `CLAUDE.md`, global output
  styles, global-scope `SessionStart` hooks, `--append-system-prompt`
  (per-invocation, no file at all), and a **live, already-used** MCP
  `instructions` channel (AgentStack's own gateway) all reach model context
  with zero bytes written under the project directory. The `instructions`
  field is genuinely dynamic — it's generated per-`initialize` call, so it
  can reflect current trust/lease state rather than being a static file.
- **Codex CLI — close, with one caveat.** Global `AGENTS.md`,
  `model_instructions_file` (full replace), and `project_doc_max_bytes`
  guardrails are all global-config-only paths. MCP `instructions` consumption
  by Codex itself is unconfirmed — the CLI is an MCP client, but no source
  found says it surfaces the field into context.
- **Gemini CLI — close for static content, strong for full override.**
  Global `settings.json` context-file config plus `GEMINI_SYSTEM_MD` (full
  system-prompt replacement via env var + external file, no project file
  needed) both work without touching the project. MCP `instructions`
  consumption is unconfirmed.
- **OpenCode — the most explicitly documented per-invocation override.** The
  documented system-prompt assembly pipeline **names** a `--system` CLI-flag
  stage as the final, highest-precedence layer, plus a global `AGENTS.md` and
  a config-level `instructions` array that can point at URLs, not just local
  files. This is the best-documented "give me dynamic instructions with no
  project file" story of the six, on paper — but MCP `instructions`
  consumption and the plugin-injection path are both unconfirmed by direct
  source in this pass.
- **Copilot CLI — global instructions exist, but is the shallowest surveyed
  here.** `~/.copilot/copilot-instructions.md` is a genuine global,
  project-file-free channel. Beyond that, its hook system is documented as
  automation/approval-gating (block/allow tool calls), not as a text-to-context
  channel, and its native instruction discovery (`.github/hooks/`,
  `.github/copilot-instructions.md`) is repo-file-first by design — the
  global path is there, but it's the harness's secondary path, not its
  primary one.
- **Cursor — weakest evidence gathered.** User Rules (global, GUI-managed)
  are real and reach the model, but the CLI agent's flag surface for
  system-prompt injection was not found in search results, and MCP
  `instructions` consumption is unconfirmed. Of the six, this is the harness
  where "zero project files" is least verified — not because it's
  necessarily impossible, but because primary-source coverage was thinnest.

**Bottom line for the ticket's actual question:** the automatic-delivery
doc's blanket claim ("MCP cannot inject these") is **not accurate as an MCP
protocol limit** — the protocol has a purpose-built `instructions` field, and
AgentStack's own gateway already uses it successfully with Claude Code today.
The accurate claim is narrower and per-harness: MCP-`instructions` consumption
is *confirmed* for Claude Code, *unconfirmed either way* for the other five in
this pass, and every harness surveyed has at least one non-MCP, non-project-file
channel (global config file, env var, or CLI flag) that already delivers
instructions dynamically-ish (global files are still static files, just not
project-committed ones; `--append-system-prompt`/`--system`/`GEMINI_SYSTEM_MD`
are the only truly per-invocation, filesystem-free options found).
