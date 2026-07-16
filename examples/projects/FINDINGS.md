# Dogfooding findings — pre-launch baseline

Date: 2026-07-16 · Binary: `agentstack 0.10.1` built from `a1aa7cd` (main) ·
Method: five runnable example projects (this directory, each with an isolated
PASS/FAIL `assert.sh`), two sandboxed investigations, and a device test on the
real machine. Every claim below was produced by a command and re-verified
independently (fresh sandbox, second run) before being written down.

**Bottom line: ready to dogfood.** The core promises hold end-to-end — one
manifest fans out correctly to native configs, secrets stay `${REF}` in the
portable artifacts, the machine policy floor cannot be loosened by a repo
through the real gateway, denials are audited, both skill delivery paths serve
identical bytes, and `run --locked` gates/swaps/restores exactly as designed.
The 12 issues filed (#11–#22) are one security-relevant gap, a family of
"silent drop" diagnostics, two discovery gaps, and four paper cuts — none
undermines the security model's core.

> **Update (same day):** all 12 issues are fixed on this branch, each pinned
> by a regression test, and every example probe that documented a defect now
> asserts the fixed behavior instead of skipping. Highlights: guard resolves
> the project manifest through `resolve_manifest_dir` (#11); both run paths
> spawn at the project root (#20); silent drops warn on every surface and
> instruction targets validate like servers' (#12–#14); `agentstack search`
> covers the central library and `list_loadable` takes a query (#17, #18);
> `--plan` agrees with live on fresh homes and `report` renders locked runs
> (#21, #22).

## Issues filed

| # | Title | Severity | Found by |
|---|-------|----------|----------|
| [#11](https://github.com/Tarekkharsa/agentstack/issues/11) | guard ignores project-layer `[policy.filesystem]` deny at the preferred `.agentstack/` location | **security** — *fixed on this branch* | restricted-folders |
| [#12](https://github.com/Tarekkharsa/agentstack/issues/12) | instructions/skills targeting an incapable adapter are silently dropped on every surface | correctness of the mental model | multi-cli-webapp, per-cli-instructions, D3 |
| [#13](https://github.com/Tarekkharsa/agentstack/issues/13) | `explain <instruction>` claims `*` compiles into *each* target's instruction file | honesty of output | multi-cli-webapp |
| [#14](https://github.com/Tarekkharsa/agentstack/issues/14) | unknown adapter id in `[instructions.*] targets` isn't validated (servers/plugins are) | validation gap | per-cli-instructions |
| [#15](https://github.com/Tarekkharsa/agentstack/issues/15) | one-manifest-demo claims the fragment reaches Cursor via AGENTS.md | docs | D3 |
| [#16](https://github.com/Tarekkharsa/agentstack/issues/16) | doctor says "no skills defined" while a profile references + materializes a library skill | misleading output | multi-cli-webapp |
| [#17](https://github.com/Tarekkharsa/agentstack/issues/17) | `agentstack search` can't see the central library at all | discovery gap | D2 |
| [#18](https://github.com/Tarekkharsa/agentstack/issues/18) | `agentstack_list_loadable` has no query param and silently ignores one | discovery gap / context cost | D2 |
| [#19](https://github.com/Tarekkharsa/agentstack/issues/19) | `lib list` omits skill descriptions | paper cut | D2 |
| [#20](https://github.com/Tarekkharsa/agentstack/issues/20) | `agentstack run` (plain and `--locked`) spawns the harness in `.agentstack/`, not the project root | **real-usage bug** — *fixed on this branch* | device test |
| [#21](https://github.com/Tarekkharsa/agentstack/issues/21) | `run --locked --plan` refuses on a fresh home where the live run succeeds (commitment key) | first-run UX | device test |
| [#22](https://github.com/Tarekkharsa/agentstack/issues/22) | `report` renders nothing for locked runs; `--json` posture is null; posture table predates HOST/PROTECTED | evidence visibility | device test |

## Deliverable 2 — how skills are indexed and searched

**Verdict: SKILL.md frontmatter descriptions drive no search anywhere.** They
are display-only. The only query-bearing surfaces match against the *embedded
catalog + remote MCP registry*, never the user's own library.

Method: isolated home, 6 library skills with bland names (`skill-a`…`skill-f`)
whose frontmatter descriptions carry unique words (quokka, zeppelin, marmalade,
obsidian, tangerine, narwhal); a project profile referencing three of them;
locked + trusted; every surface probed, then re-run in a second fresh sandbox
(byte-identical outcomes).

| Surface | Covers library skills? | What text drives matching |
|---|---|---|
| `agentstack search` (CLI) | **No** | name+description+tags of the *embedded catalog* + MCP registry only (`provider::search_all` has no library provider) |
| `agentstack_search` (MCP) | **No** | same code path |
| `lib list` | Yes (dump) | none — and no description column ([#19](https://github.com/Tarekkharsa/agentstack/issues/19)) |
| `agentstack_list_loadable` (MCP) | Yes (dump) | none — empty inputSchema; a `query` arg is silently ignored ([#18](https://github.com/Tarekkharsa/agentstack/issues/18)); descriptions ARE included in the listing |
| `tools_search` (MCP) | No (by design) | runtime proxied MCP tools only, never skills |
| `explain <name>` | Yes | exact-name lookup only; `explain quokka` errors |

The sharpest evidence: `agentstack search skill-a` (an exact library skill
name) returns `xskill-ai`, an unrelated *remote registry* server that
substring-matches — while the user's own `skill-a` is invisible. And
`agentstack_list_loadable {"query":"quokka"}` returns the identical unfiltered
7-entry list.

**Recommendation.** Index frontmatter, in two steps: (1) add the central
library as a search provider so `agentstack search` matches library skill
names + frontmatter descriptions ([#17](https://github.com/Tarekkharsa/agentstack/issues/17))
— the description is already parsed for `explain`/`list_loadable`, so this is
plumbing, not new parsing; (2) give `agentstack_list_loadable` an optional
query over the same fields ([#18](https://github.com/Tarekkharsa/agentstack/issues/18)).
Tags could follow later; description keywords are the 90% win. At today's
13-skill library the unfiltered dump is tolerable; it stops scaling quietly —
every discovery call ships the whole library's descriptions into agent
context, and the cost grows with every `lib add`.

Positive finding: profile leases correctly fence the loadable *set* (a
`backend` lease exposed exactly its three skills + the built-in
`using-agentstack` manual).

## Deliverable 3 — how capabilities reach each CLI

One manifest targeting four CLIs: an http server with a `${REF}` header, a
stdio server with `env`+`cwd`, one skill, one instruction fragment.

| Capability | Claude Code | Codex | Cursor | Gemini CLI |
|---|---|---|---|---|
| MCP config | ✅ `.mcp.json`, `type:"http"`/`"stdio"` tags | ✅ `.codex/config.toml`, `http_headers` sub-table, no transport tag | ✅ `.cursor/mcp.json`, transport inferred (no `type`) | ✅ `.gemini/settings.json`, **`httpUrl`** not `url` |
| Skills (static render) | ✅ `.claude/skills/` symlink | ✅ `.agents/skills/` symlink | ❌ **silently nothing** | ✅ `.gemini/skills/` symlink |
| Skills (MCP/gateway load) | ✅ | ✅ | ✅ (any MCP client) | ✅ |
| Instructions | ✅ `CLAUDE.md` | ✅ `AGENTS.md` | ❌ **silently nothing** | ❌ **silently nothing** |
| stdio `cwd` | ⚠ no native key — rewritten to `sh -c "cd … && exec …"` | ✅ native `cwd` | ✅ native `cwd` | ✅ native `cwd` |

Surprises worth knowing during the week:

- **The silent drops are the headline** ([#12](https://github.com/Tarekkharsa/agentstack/issues/12)):
  `instructions --target cursor` reports "0 instruction file(s) would change"
  and exits 0; `use` counts "1 skill(s)" in its header while Cursor's block
  silently gets none. Both Cursor (`AGENTS.md`, `.cursor/rules`) and Gemini
  (`GEMINI.md`) natively support instruction files — these are adapter
  descriptor gaps agentstack could close, not CLI impossibilities.
- Claude Code's `sh -c` cwd rewrite is functional but changes the process tree
  (launcher is `sh`, not `node`), and doctor's Quirks section doesn't mention
  it — while it *does* flag the bare-`node`-on-PATH quirk for the same server.
- Codex reading `.agents/skills` is documented only in the adapter descriptor
  comment, not in `docs/` — a user reading only the docs wouldn't learn Codex
  gets skills at all.
- agentstack renders config for CLIs that aren't installed (Cursor "not
  detected" yet `.cursor/mcp.json` written) — reasonable for teams, worth
  knowing.
- doctor helpfully flags that Codex will *ignore* a rendered
  `.codex/config.toml` until the project is trusted in `~/.codex/config.toml`
  (`projects.<path>.trust_level`) — a real cross-tool gotcha caught well.

## Deliverable 4 — device test (real machine)

Read-only against the real environment: orientation (8 CLIs detected),
`doctor --manifest-dir ~/agent-setup` (0 errors, 20 warnings — all drift in
the maintainer's own setup, e.g. 9 pending changes per CLI and stale
instruction regions; worth an `apply --write` + `instructions --write` pass
before the week starts), `guard status` (enabled, hooks in 9 CLIs),
`guard test rm -rf /` → DENY, `trust --list` clean.

`run --locked` (trusted scratch project, isolated home, real `claude` binary):

- Posture banner `HOST / PROTECTED` with honest-limits text: ✅
- Fail-closed gates recorded (trust → strict lock verify → policy admission →
  grant frozen `sha256:…`): ✅, all in `runs/<id>/events.jsonl`
- **Gateway-only swap confirmed from inside the harness**: during the run,
  project `.mcp.json` contains only the `agentstack mcp --auto-project` bridge
  entry; the pre-existing hand-written config is parked outside the repo
- **Byte-identical restore**: sha256 of `.mcp.json` identical before/after: ✅
- Defects: plan-vs-live disagreement on fresh homes
  ([#21](https://github.com/Tarekkharsa/agentstack/issues/21)), harness cwd
  ([#20](https://github.com/Tarekkharsa/agentstack/issues/20)), report
  rendering ([#22](https://github.com/Tarekkharsa/agentstack/issues/22)).

`--locked --profile` and `--locked --sandbox` refuse with clear
"not wired yet" messages — documented limitations, not defects.

## Questions for the maintainer (not filed as issues)

1. **`mcp --manifest-dir` serves untrusted projects.** Explicitly documented
   ("naming a directory is the consent", docs/reference.md) and consistent
   with plain `agentstack mcp` — but in tension with CLAUDE.md rule 3's
   "untrusted means inert, no exceptions". The policy-intersection example had
   to use `--auto-project` to demonstrate the trust gate at all. If
   consent-by-invocation is intended, consider saying so next to rule 3;
   harness configs that hardcode `--manifest-dir` bypass the gate forever.
   Related: the names-only untrusted listing (descriptions hidden until trust)
   also doesn't engage under bare `--manifest-dir`.
2. **Profile `skills = ["*"]` expands to inline manifest skills only** — it
   never sweeps in central-library skills. Defensible (a wildcard over the
   manifest, not the machine's library), but users may expect "everything
   available"; skills-workout documents and asserts the actual semantics.
3. **Real-machine drift**: doctor shows pending changes + stale instruction
   regions across 7 CLIs on the real setup — run the suggested fixes (or
   `adopt`) before the dogfooding week so day-one signal is clean.

## Positive findings (things that just worked)

- Policy intersection through the real gateway: machine `"*" = ["!delete_*"]`
  made the repo's own `delete_everything` allowance moot — the tool is
  invisible to `tools_search`, refused by name with the machine layer named in
  the error, and both the denial and the allowed call land in
  `audit/calls.jsonl` with correct outcomes.
- Guard denials ARE auditable — file-tool denials record as
  `server='host-guard', tool='read: <path>'` (stronger than guard-demo
  advertises).
- Both skill delivery paths byte-identical: static symlink render vs
  `agentstack_load` over a lease, for inline and library skills alike.
- Never-clobber holds: a hand-made unmanaged dir in `.claude/skills/` survives
  profile switches and prunes.
- Secrets: resolved values appear only in native configs; manifest + lock
  stayed placeholder-only in every project, asserted every run.
- Instruction managed regions preserve surrounding hand-written prose
  byte-exactly (per-cli-instructions asserts the byte-prefix).
