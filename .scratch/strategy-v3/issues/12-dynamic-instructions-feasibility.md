# Dynamic instructions: what can each harness accept at session start?

Type: research
Status: resolved

## Question

The maintainer challenged the claim that instructions (and settings) cannot be delivered dynamically. Verify it per harness instead of asserting it: for Claude Code, Codex CLI, Gemini CLI, Cursor, Copilot CLI, and OpenCode, what mechanisms exist to inject instruction/system-prompt content at session start without writing files into the project — MCP initialize `instructions`, system-prompt flags, user/global-scope config, output styles, hooks, env vars? For each: does it reach the model's context, at what precedence, and with what size limits? Also: can any harness vary instructions by model (the per-model instruction pain), and what model-detection hooks exist? Output: a capability matrix with sources, and an honest verdict on how close to "zero project files" instructions can get per harness.

## Answer

- Claude Code — closest to zero-project-files: MCP `initialize` `instructions` field is confirmed live (AgentStack's own gateway already uses it), plus global CLAUDE.md, output styles, SessionStart-hook `additionalContext`, and `--append-system-prompt`.
- Codex CLI — close via global-only paths: `~/.codex/AGENTS.md`, `model_instructions_file` (full replace), `project_doc_max_bytes`; MCP `instructions` consumption by Codex itself is unconfirmed.
- Gemini CLI — close: global `settings.json` context files plus `GEMINI_SYSTEM_MD` env var for a full system-prompt override, no project file needed; MCP `instructions` consumption unconfirmed.
- Cursor — weakest evidence: global User Rules reach the model, but no documented CLI system-prompt flag and MCP `instructions` consumption unconfirmed.
- Copilot CLI — shallow: global `~/.copilot/copilot-instructions.md` is real, but hooks are documented as approval-gating, not text-injection, and its native instruction discovery is repo-file-first.
- OpenCode — best-documented per-invocation override: a `--system` CLI flag is the explicit final stage of its system-prompt assembly pipeline, plus global AGENTS.md and a URL-fetchable `instructions` config array.
- No harness has native per-model instruction branching; Claude Code's SessionStart hooks receiving an (unguaranteed) `model` field is the closest side channel found anywhere.
- Full findings: branch `research/dynamic-instructions`, file `docs/design/research/dynamic-instructions-2026-08.md`.
