# Competitive watch refresh: vercel/eve

Type: research
Status: resolved

## Question

Has vercel/eve tripped any of v2's four tripwires since 2026-07-31? The tripwires: (1) eve imports existing CLI setups; (2) eve renders or exports configuration to other harnesses; (3) eve's registry becomes de-facto skill distribution; (4) eve ships a content-bound trust gate. A tripwire firing warrants a strategy revisit and changes what v3 must say about the competitive landscape. Also note any new entrants in the agent-capability-management space worth naming.


## Answer

1. eve imports existing CLI setups — **not tripped**: parent-process detection (`agent-detection.ts`, `eve init` REPL hand-off) only detects/launches, never reads or imports Claude Code/Codex/Cursor config.
2. eve renders/exports its configuration to other harnesses — **not tripped**: no mechanism found in README, docs, or full CHANGELOG; the only adjacent feature (generated AGENTS.md) runs the opposite direction (tells other agents how to call eve, not the reverse).
3. eve's registry becomes de-facto skill distribution — **ambiguous, trending**: eve's own registry now points AGENTS.md at itself for other coding agents to consume, but the tool actually doing cross-harness distribution today is the sibling project vercel-labs/skills (npx skills, 27.8k stars, 75+ target agents including Claude Code/Codex/Cursor), which eve's own docs recommend as its skill installer.
4. eve ships a content-bound trust gate — **not tripped**: `eve add` still runs behind a single y/n confirm (no hash/signature); vercel-labs/skills has an open, unmerged RFC (#617, since 2026-03-13) for optional signature verification, explicitly not shipped.

Branch: `research/eve-watch-2026-08`
Findings: `docs/design/research/eve-watch-2026-08.md`
