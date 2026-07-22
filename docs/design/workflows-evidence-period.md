# D7 workflows — evidence period (§9.4 witness)

Date: 2026-07-21 · agentstack v0.15.0 (built from `648d227`) · macOS host, HOST/PROTECTED tier throughout.

This file is the witness the W3 gate asked for: **two real recurring maintainer
tasks driven through the interim orchestrator** — Claude Code's native Workflow
tool, every `agent()` step a thin courier (sonnet, low effort) into
`agentstack run <harness> --locked --prompt` (the W2 primitive). Nothing here
is synthetic benchmark work; both tasks are chores this repo actually needs
run, and their findings are reported as findings, not ceremony.

Sections: [0. setup](#0-setup--phase-0-smoke) · [1. Task A](#1-task-a--docs-claim-consistency-sweep) ·
[2. Task B](#2-task-b--security-lens-audit-of-cratesegress) · [3. evidence tree](#3-evidence-tree-per-child-run) ·
[4. friction list](#4-friction-list) · [5. verdict](#5-verdict-against-94)

> **Closure status (same commit as this file).** The findings below were acted
> on rather than filed: the §2 IPv6 anti-SSRF gap is fixed in
> `crates/egress/src/netguard.rs` (6to4, Teredo, benchmarking, and ORCHID
> prefixes now refused inside `2000::/3`, with witnesses); friction **F1** and
> **F3** are fixed in `run --locked`; and the §1 doc bugs are corrected at
> their sources. The findings are left stated as found — this note records what
> happened to them. **F2 remains open and is the W3 design-gate item.**

## 0. Setup + Phase 0 smoke

**Trust preconditions found, not assumed.** `agentstack doctor` (from
`~/agent-setup`): 0 errors, 5 warnings — pre-existing drift (Codex hand-edit
pending `adopt`/`apply`), nothing re-gated by the recent commits. The trust
store held only two entries, both stale: `gha-tanstack` (drifted) and the
`w2smoke` scratch project from the W2 review session, whose directory a
scratchpad cleanup had deleted. **There was no valid trusted project on the
machine, and the agentstack repo itself had no manifest** — the evidence
period had to create one: a minimal empty-surface manifest at
`.agentstack/agentstack.toml` in the repo root (untracked, uncommitted),
trusted at `sha256:c9478d0dbbdc4891d5074d8f404442a7368c90b677893d8bec6930cb1ca51caa`.

Smoke matrix (prompt `reply with exactly: ok`; stdout verified byte-exact with
`cat -ev`, banners confirmed on stderr):

| run id | harness | project | grant digest (sha256:) | output | outcome |
|---|---|---|---|---|---|
| r-3872d119c9 | claude-code | scratch evproj | `a567a8a1…7fb6955` | 3 B · `dc51b8c9…` | exit 0 · 6961 ms |
| r-888502ab9b | codex | scratch evproj | `1ad1515a…04aa573b` | 0 B · `e3b0c442…` (empty) | exit 1 · 276 ms |
| r-7d5a24f2ca | claude-code | repo root | `c02abb2d…e14cb621` | 3 B · `dc51b8c9…` | exit 0 · 6066 ms |
| r-945ce356f3 | codex | repo root | `56096645…d9280297` | 3 B · `dc51b8c9…` | exit 0 · 12313 ms |
| r-c8096a9cd6 | claude-code | repo root (concurrency A) | `533f76bc…48cd507c` | 2 B · `06f961b8…` | exit 0 · 4851 ms |
| r-5eb45c8689 | claude-code | repo root (concurrency B) | `51e5a3a9…1780d010` | — | **refused** · 15 ms |
| r-50a622bc75 | codex | scratch evproj (exit-code check) | `1ad1515a…04aa573b` | 0 B · empty | exit 1 · 307 ms |

What the smoke established, beyond "it works":

- **stdout hygiene holds under real piping.** `cat -ev` shows exactly `ok$` —
  the W2 stderr-banner review follow-up survives contact with an orchestrator.
- **The grant binds project + prompt, deterministically.** The same prompt in
  two different projects froze different digests (`a567a8a1…` vs
  `c02abb2d…`); the same prompt in the same project froze the *same* digest
  twice (`1ad1515a…` for both codex evproj runs). Exactly the property the
  evidence tree needs.
- **`env -u CLAUDECODE` was NOT needed.** Nested `claude -p` under the
  inherited Claude Code environment (orchestrator → courier subagent → Bash →
  agentstack → claude) behaved normally in every run. The prompt's feared
  env-inheritance failure did not materialize on this machine/version.
- **Child cwd is the manifest project dir, not the invocation cwd** (verified
  with a `pwd` child). This is why the repo needed its own manifest: children
  must sit in the repo to audit it.
- **Concurrent locked runs in one project are mutually exclusive.** Run B
  failed closed pre-launch — `another locked run (r-c8096a9cd6) is scoping
  .mcp.json` — and was recorded as `refused · 15 ms`. Honest, but it removes
  all map-phase parallelism (friction F2).
- **Child failure does not reach the parent exit code.** The codex git-check
  failure was recorded `completed · exit 1` in the report, but the
  `agentstack` process exited 0 (friction F3).
- **Two real bugs found before the tasks even started** (friction F1, F4):
  the codex scope guard requires a pre-existing `.codex/` directory, and
  codex refuses non-git project dirs with no way to pass
  `--skip-git-repo-check` (W2 correctly refuses trailing args with
  `--prompt`, so the flag is unreachable).

## 1. Task A — docs claim-consistency sweep

**Shape:** map (one locked child per page, `ENFORCEMENT.md` via codex) →
JS shuffle (parse `CLAIM/VERDICT/EVIDENCE` blocks, keep non-OK) → reduce
(one child ranks discrepancies) → verify (refute-framed child per HIGH).
**Coverage:** 4 pages, 113 testable claims checked against code + live CLI,
13 non-OK, 1 HIGH survived refute-verification. Every finding below was
re-confirmed by me against the code before writing it here.

**Findings (all against the actual repo, verdicts are mine after re-check):**

1. **[HIGH — confirmed] `docs/concepts.md` undercounts the machine-policy
   summary states.** The page (lines 149-151) says `doctor` prints one of
   `open`, `restrictive`, or `mixed`. `classify_machine_posture`
   ([doctor.rs:2038](../../crates/cli/src/commands/doctor.rs)) returns **six**
   values — it returns `unconfigured`, `degraded`, and `blocked` *before* ever
   reaching those three. `unconfigured` ("no machine policy file") is the state
   every machine without `~/.agentstack/agentstack.toml` gets, and `degraded` /
   `blocked` are the security-relevant last-known-good and fail-closed states.
   The doc omits exactly the states a reader most needs to recognize. The
   verify child confirmed it; the unit tests at doctor.rs:2520 assert those
   strings. `docs/reference.md` documents them correctly — concepts.md is the
   outlier.

2. **[MEDIUM — confirmed] `docs/concepts.md` points at the wrong home for the
   posture labels.** Lines 143-146 say the four labels (`HOST / ADVISORY` …
   `LOCKDOWN / ENFORCED`) "are defined in [ENFORCEMENT.md — the matrix]". The
   `#the-matrix` anchor resolves (ENFORCEMENT.md:89), but that matrix is keyed
   by *mode* (`host` / `gateway` / `--sandbox` / `--lockdown`) and never
   enumerates the label strings — `grep` for them in ENFORCEMENT.md returns
   zero hits. The labels are actually defined in `docs/reference.md` §
   Execution posture. A reader who follows the link to learn what the labels
   guarantee finds no definition.

3. **[MEDIUM — confirmed] `restrictive` is described more narrowly than the
   code applies it.** concepts.md:151 says `restrictive` "flags a rename-proof
   `"*"` rule". The classifier fires it on `has_wildcard ||
   !policy.filesystem.is_empty()` (doctor.rs:2072) — a `[policy.filesystem]`
   scope alone earns `restrictive` with no `"*"` rule anywhere. doctor.rs's own
   message already says "(or a filesystem scope)"; concepts.md is the stale copy.

4. **[MEDIUM — confirmed against ARCHITECTURE.md] the crate-dependency table is
   wrong for `executor`.** `docs/ARCHITECTURE.md` (dep table, ~line 486) lists
   `executor → core, runtime, recorder`. `crates/executor/Cargo.toml` has **no
   internal agentstack edges at all** (only serde/serde_json/sha2/thiserror).
   The map child that found this returned in a corrupted stdout format (see
   §4-F5) so it was invisible to the reduce step; recovered from the on-disk
   child output. Note this contradicts CLAUDE.md's own stated edge
   `executor → core, runtime, recorder` — either the code dropped the edges or
   the architecture is mis-documented; worth a maintainer decision.

5. **[MEDIUM — confirmed against ARCHITECTURE.md] `[policy.filesystem]` doc
   omits the `deny` field.** ARCHITECTURE.md (~line 270) lists `read`/`write`
   as the filesystem key set; `FsPolicy`
   ([model.rs:200](../../crates/core/src/manifest/model.rs)) has a third field
   `deny`, compiled via `fs_deny_layer`. The host-guard deny dimension is
   undocumented in that page.

6. **[MEDIUM — confirmed against ENFORCEMENT.md] a stale symbol name.**
   ENFORCEMENT.md (lines 125-128, 184-186) refers to a `Gateway::from_plan`
   constructor; the code calls `Gateway::from_frozen`
   ([sandbox.rs:900](../../crates/cli/src/commands/sandbox.rs)); `from_plan` has
   no definition. (From the codex-run ENFORCEMENT map child.)

7. **[LOW] the generated command inventory under-reports two hidden verbs.**
   `docs/reference.md` "All commands" lists `guard test/install/uninstall/status`
   and `self link/which` but omits `guard check` and `self docs` — both
   `#[command(hide = true)]` maintainer/hook entrypoints (cli.rs:624, cli.rs:327).
   The generator marks hidden *top-level* commands but silently drops hidden
   *subcommands*, so the "full command surface" framing overstates. Consistent
   behavior, minor framing bug.

**One reported discrepancy the reduce child correctly discarded as spurious:**
a claimed dead link `reference.md#where-rendered-files-live-three-modes` — the
target has a hand-placed `<a id="where-rendered-files-live-three-modes">`
anchor at reference.md:370 that a heading-slug-only checker missed. Good
negative result: the reduce step caught a mapper false positive.

**`ARCHITECTURE.md` was checked at 26 claims / 23 OK and `ENFORCEMENT.md` at
25 / 18 OK** — both had their non-OK claims silently zeroed by the shuffle
parser (§4-F5) and were recovered by hand from the on-disk outputs. Without
that recovery, findings 4-6 would have been lost — a direct hit on the "no
structured output" friction.

## 2. Task B — security-lens audit of crates/egress

**Shape:** map (3 children, distinct lenses; dependency lens via codex) →
JS shuffle (collect `FINDING/SEVERITY/EVIDENCE`) → reduce (rank, merge) →
verify (refute-framed child per HIGH). **20 raw findings → 14 ranked → 2 HIGH
verified.** This is a real weekly maintainer chore and it produced a genuine
security finding.

**The headline finding (HIGH, verify-CONFIRMED, and I re-confirmed it against
the code myself):**

> **`is_forbidden_v6` is not the deny-by-exclusion it claims to be.**
> [netguard.rs:57-66](../../crates/egress/src/netguard.rs) admits *all* of
> global unicast `2000::/3` (`(s[0] & 0xe000) == 0x2000`) and carves back only
> `2001:db8::/32`. But `2000::/3` contains two special-purpose sub-ranges that
> encapsulate arbitrary IPv4: **6to4 (`2002::/16`)** and **Teredo
> (`2001:0000::/32`)**. `2002:a9fe:a9fe::` is the 6to4 encapsulation of
> `169.254.169.254` — the cloud metadata IP — and it passes the guard. The
> in-code comment explicitly promises this function "Truly deny-by-exclusion
> for v6", so this is a correctness gap against a stated security property, in
> the anti-SSRF path a locked-down sandbox relies on
> (`resolve_validated`, proxy.rs:357).

Severity is bounded by two preconditions — a policy-*allowed* name must
resolve into those ranges (or a 6to4/Teredo literal must be allowed), and the
host must have a 6to4/Teredo tunnel to actually decapsulate — so it is not
trivially exploitable on a modern host with those tunnels disabled. But it is
a real hole in a deny-by-exclusion claim, cheap to close (reject `2002::/16`,
`2001:0000::/32`, and reconsider whether `2000::/3`-only is the right
allowlist at all), and it is exactly the class of defense-in-depth bug this
weekly audit exists to catch. **This finding alone repays the evidence
period.**

**The second HIGH was correctly REFUTED by the verify stage** — a good
demonstration that the validation reducer earns its place:

> *Claimed:* the ruleset version gate fails closed only on `version >
> RULESET_VERSION`, so a stale v1/v2 artifact would be accepted and enforced
> with `gateway_only_hosts` defaulted empty (D4 fence gone) — policy widening.
> *Refuted:* no producer of a stale artifact exists. Every in-process guard is
> stamped with the current `RULESET_VERSION` at compile time, the sidecar file
> is serialized fresh in the same run, and the one persisted path (the
> run-grant handoff) requires byte-equality with a fresh machine∩project
> recompile and fails closed on `version != RULESET_VERSION` (both directions).
> The "v1/v2 re-read under v3" scenario has no way to arise. A plausible-
> sounding finding that the refute stage killed — the §7 mitigation working.

**Confirmed MEDIUM findings worth a maintainer look (not verify-gated, but
code-cited and credible):**

- **Recorder shedding under flood breaks "nothing trusted runs unobserved".**
  `SpoolSender::send` ([spool.rs:121](../../crates/egress/src/spool.rs)) drops
  egress decision events once the 1024-deep queue fills, with a one-time
  `eprintln!` and **no per-run count of what was lost**. The queue's fill rate
  is fully attacker-controlled (one event per CONNECT), so a container can
  flood cheap CONNECTs to make a chosen later allow/block be *enforced but
  unrecorded*. The enforcement still holds; the audit trail is what silently
  degrades. A dropped-count in the run report would make the loss visible.
- **The 502 dial-failure arm records `allowed: true` for a connection that was
  never established.** proxy.rs:241 emits the held `decision.event` before
  writing `502 Bad Gateway`, so `events.jsonl` shows a successful egress allow
  for a failed dial — contradicting the "exactly one event reflecting the
  FINAL outcome" invariant stated three lines above, and overstating what run
  reports say the sandbox actually reached.
- **`execution_relay::serve` has no read/idle timeout.**
  [execution_relay.rs:170](../../crates/egress/src/execution_relay.rs) — a
  runtime that opens all 8 `MAX_CONNECTIONS` and never sends a newline pins
  every relay slot forever; the per-frame byte cap bounds memory but not time,
  unlike the proxy's `STEP_TIMEOUT`.
- **`allow_local_targets` disables the whole netguard address-class check from
  a plain env var**, and nothing refuses it in combination with `lockdown:
  true` — the strongest posture can run with anti-SSRF off, no event, no
  READY-line signal (egress_proxy.rs:91 → proxy.rs:351).
- **A duplicate-SNI ClientHello can clear the domain-fronting guard.**
  `parse_sni` returns on the *first* `server_name` extension while the proxy
  replays the raw ClientHello upstream (sni.rs:142, proxy.rs:283), so a benign
  SNI followed by an attacker's clears the guard against any lenient upstream.

**Dependency + unsafe lens (codex) — mostly a clean bill, with one workspace-
rule discrepancy:** `crates/egress` correctly `#![forbid(unsafe_code)]`s (the
only "unsafe" tokens are the two forbid attributes), has no `build.rs`, no
feature flags, no optional deps. But the child flagged that **CLAUDE.md's claim
that tokio is "confined to the egress crate" is contradicted by
`crates/cli/Cargo.toml` and `crates/runtime/Cargo.toml`**, which also depend on
tokio. This is a doc-vs-reality gap in the non-negotiable-rules section worth
reconciling (either the rule is aspirational or the wording needs to say
"async egress enforcement" rather than "confined").

## 3. Evidence tree (per child run)

Grant digest, output bytes + sha256, and duration are pulled from
`agentstack report run <id> --json` (the `grant_frozen`, `headless_output`,
and `locked_outcome` events) — i.e. from the recorder's own evidence, not from
the courier's self-report. All 19 child runs (7 Phase-0 smoke + 12 workflow)
are recorded; none were skipped or errored at the courier layer.

### Phase 0 smoke (7 runs) — see the table in §0.

### Task A + B workflow children (12 runs)

| id | harness | run id | grant digest (sha256:) | output | exit | dur |
|---|---|---|---|---|---|---|
| A-map-reference | claude-code | r-b35c4da177 | `8f3f54c0…ae7e6154` | 9484 B · `e9352a91…` | 0 | 203 s |
| A-map-concepts | claude-code | r-eff62d5e82 | `aabc6177…2fc8b13d` | 10947 B · `6895ebd5…` | 0 | 220 s |
| A-map-architecture | claude-code | r-00268391bb | `94911399…421d89f8` | 8012 B · `cbc9c4ca…` | 0 | 195 s |
| A-map-enforcement | **codex** | r-79a395bac6 | `f246c59c…be9f3a4ca` | 7648 B · `d7162dba…` | 0 | 438 s |
| A-reduce | claude-code | r-95b23e5206 | `fd72c0af…b110adfa` | 3159 B · `3d772745…` | 0 | 91 s |
| A-verify-1 | claude-code | r-7b54e60a94 | `3f828713…4b81ec2a` | 816 B · `56d6effe…` | 0 | 36 s |
| B-map-hostile-input | claude-code | r-8f6a73bb9c | `121fc8ed…7d640cf4b` | 3413 B · `90a6cb2e…` | 0 | 313 s |
| B-map-policy-narrowing | claude-code | r-b5b3c9b8a8 | `6da8e502…d5452ec70` | 4333 B · `c961c988…` | 0 | 248 s |
| B-map-deps-unsafe | **codex** | r-288ae9a903 | `93f31d34…04e5add85` | 2044 B · `aa48b4de…` | 0 | 127 s |
| B-reduce | claude-code | r-5ab3d3ca8f | `e52a9422…91da144dc` | 7194 B · `667c413f…` | 0 | 114 s |
| B-verify-1 | claude-code | r-85217457c0 | `704232c9…59d411a638` | 889 B · `f05e444f…` | 0 | 57 s |
| B-verify-2 | claude-code | r-93258e3428 | `99e092d2…099538c13` | 737 B · `43e10fa3…` | 0 | 116 s |

Notes:
- **All 12 grant digests are distinct** — each child froze a grant binding its
  own (project, argv-with-prompt) tuple. The evidence chain the design promised
  holds end to end: prompt → argv → frozen grant → recorded outcome, per child.
- **Both codex children succeeded** (`codexDown: false`) — the cross-CLI claim
  is proven, not just asserted. Codex was ~2× slower per child (438 s / 127 s)
  and the ENFORCEMENT child was the single longest run of the period.
- **Total child wall time ≈ 38 min** across the 12 (sum of durations), but the
  *workflow* wall was 47 min because every locked child ran **strictly
  serialized** — the concurrency lock (§0, §4-F2) forbids two locked runs in
  one project at once, so the map fan-out that the script expresses as parallel
  executed one-at-a-time.
- The full per-child stdout is preserved on disk at
  `scratchpad/ev/out/<id>.out` and the exact prompt at
  `scratchpad/ev/prompts/<id>.txt` (both scratchpad, not committed).

## 4. Friction list

Every place the interim path hurt, honestly, each tagged. "W3 fixes this by
design" = the engine as specified removes it; "W3 design gap" = the engine as
specified does *not* remove it and needs a decision; "not W3's problem" =
environmental or my-harness, out of scope.

**F1 — codex needs a pre-existing `.codex/` directory or the scope guard
crashes.** First codex smoke died with `creating the scope guard
.../.codex/config.toml.agentstack-locked.lock: No such file or directory`.
`mkdir -p .codex` fixed it. The locked-run scope-guard code assumes the
harness's config parent dir exists. **→ not W3's problem** — it's a W2 bug in
`run --locked`. Should `create_dir_all` the guard's parent, or fail with a
clear "run this harness once first" message. Small, worth fixing regardless of
W3.

**F2 — one locked run per project at a time; the map phase cannot parallelize.**
The second concurrent locked run in the same project fails closed:
`another locked run (r-…) is scoping .mcp.json`. This is *correct* for the W2
primitive (it swaps the project's `.mcp.json` for the run's lifetime, which is
inherently exclusive), but it means the entire map→reduce→verify shape runs
serialized — the 12 children took 47 min wall where true fan-out would be
~8-10. **→ W3 design gap — needs a decision.** The engine's whole value
proposition is fanning children out; if each child is a `run --locked` that
mutates the shared project config, they serialize. Either children run under a
per-child ephemeral config/workspace (not the project's `.mcp.json`), or the
engine needs a different admission that grants N children one frozen ruleset
without N config swaps. This is the single most important thing the evidence
period surfaced about W3's design.

**F3 — the child's exit code does not propagate to `agentstack`'s exit code.**
The failed codex smoke (`git-repo-check`) recorded `completed · exit 1` in the
report, but the `agentstack` process exited **0**. A courier that keys off
`$?` sees success. My courier had to grep the report/stderr to detect failure.
**→ W3 fixes this by design** — the engine consumes the recorded outcome
directly (not a subprocess exit), so per-child success/failure is structured.
But for W2-as-shipped (CI, scripts), this is a real footgun: a scripted locked
run that fails looks like it passed. Worth deciding whether `run --locked
--prompt` should mirror the child's exit code.

**F4 — codex refuses a non-git project dir and `--prompt` (correctly) blocks
the escape hatch.** The scratch `evproj` wasn't a git repo, so codex exited 1
with `Not inside a trusted directory and --skip-git-repo-check was not
specified`. W2 refuses trailing harness args with `--prompt` (a deliberate,
correct security decision per the W2 review), so there is no way to pass
`--skip-git-repo-check` through. The fix was to run children from the repo root
(a git repo). **→ W3 design gap — minor.** Some harnesses need per-invocation
flags that aren't the prompt. The headless descriptor's `args` are fixed
literals + `{prompt}`; there's no channel for "this harness, this run, also
needs flag X". Either the descriptor grows a way to express harness-required
flags, or workflows document "children run in a git repo". Low urgency.

**F5 — no structured output channel; the courier hand-copies bytes and 2 of 4
corrupted them.** This is the big one for the map→reduce contract. Couriers
returned the child's stdout as a JSON string. Two couriers (`reference`,
`concepts`) returned it clean; two (`architecture`, `enforcement`) returned it
in Read-tool `cat -n` format (each line prefixed `N\t`), so my shuffle parser —
looking for lines starting `CLAIM:` — parsed **0 claims** from both, silently
dropping findings 4-6 of Task A. They were only recoverable because each child
*also* wrote its output to a file on disk that I could re-parse by hand. **→ W3
fixes this by design** — the engine returns each child's output as a
first-class value into the script (the design's `agent()` returns the child's
text directly, and a schema option validates structure), eliminating the
courier round-trip entirely. This is the clearest empirical case *for* building
W3: the interim path's map→reduce is only as reliable as an LLM copying bytes
through JSON, and it demonstrably wasn't.

**F6 — the report tree was assembled by hand.** There is no `agentstack
workflow report` that shows the map/reduce/verify children as one evidence
tree; I ran `report run <id> --json` 19 times and built §3's table in Python.
The per-run evidence is excellent (grant digest, output sha, outcome), but
there is no *workflow-level* rollup. **→ W3 fixes this by design** — §10 W3
lists "`workflow run` / `report` tree" as a deliverable. Confirmed as a real
need: without it, the evidence tree the gate asks for is manual labor.

**F7 — no shared budget or ceiling across children.** Each `run --locked
--prompt` is an independent process with its own (unmetered, here) cost. There
was no `max_agents`, no `max_wall_seconds`, no token ceiling spanning the 12
children — nothing would have stopped a runaway map from spawning hundreds. The
native Workflow tool's `budget` counts *its* tokens, but that is invisible to
the codex children and does not gate the locked runs. **→ W3 fixes this by
design** — ceilings (`max_agents` / `max_wall_seconds`) under `MachineLimits`
discipline are a core W3 feature (§3.1, §10). Confirmed necessary: the interim
path has no ceiling at all.

**F8 — trust setup was a real cliff, and it's load-bearing.** There was no
trusted project on the machine and the repo had no manifest; I had to `init` +
hand-edit a minimal manifest + `trust` before any child could run, and hit the
`computer-use` server's un-pinnable `cwd: "."` on the way (had to strip the
server list to empty). A first-time user of `run --locked --prompt` faces this
same wall. **→ not W3's problem** — it's the W2/trust onboarding surface, but
it *gates* every workflow, so W3's docs must front-load "you need a trusted
project first" or the engine is unreachable.

**F9 — stdout hygiene held; env inheritance was a non-issue.** The two things
the prompt flagged as likely friction did **not** bite: stdout was byte-exact
(`cat -ev` → `ok$`) with banners cleanly on stderr, and nested `claude -p` under
the inherited `CLAUDECODE` env worked without the `env -u CLAUDECODE` fallback.
Recording as friction-that-wasn't, per the instruction to record honestly:
the W2 stderr-banner follow-up and env handling are solid. No W3 action.

## 5. Verdict against §9.4

**Does this evidence justify building W3? Yes — and more pointedly than a
"it ran" pass would.**

§9.4 set the bar as: *run at least two real recurring tasks through the interim
path (native orchestrator + `run --locked --prompt` couriers) before W3
begins; if the interim path is not actually used repeatedly, the engine is not
built.* Both tasks ran, both were genuine maintainer chores (not benchmarks),
and both produced findings worth acting on:

- Task A found **7 real doc-vs-code discrepancies**, one HIGH (concepts.md
  hides the `unconfigured`/`degraded`/`blocked` machine-policy states users
  most need), plus a CLAUDE.md-vs-code dependency-edge contradiction worth a
  maintainer ruling.
- Task B found a **genuine anti-SSRF gap** (`is_forbidden_v6` admits 6to4 and
  Teredo despite a "truly deny-by-exclusion" comment) that I independently
  confirmed against the code, plus several credible MEDIUM observability/robustness
  findings — and the validation reducer *correctly killed* a plausible-but-false
  HIGH (the stale-ruleset claim), demonstrating the §7 mitigation working on
  live output.

The interim path is not just usable — it is **useful today**, which is the W2
standalone-value claim confirmed. But the evidence period also demonstrated
*why the engine is worth building* by showing exactly where the interim path
is load-bearing-but-fragile:

1. **F5 (no structured output)** silently dropped a third of Task A's findings
   through byte-copy corruption. Map→reduce reliability currently depends on an
   LLM faithfully copying text through JSON, and it demonstrably failed 2-of-4.
   This is the strongest single argument for W3: the design's first-class
   `agent()` return value + schema validation removes the failure mode
   entirely.
2. **F2 (serialization)** means the interim path gets none of the parallelism
   that is the entire point of a fan-out orchestrator — and it is also a
   **genuine W3 design question**, not just an interim wart: N locked children
   that each swap the project's `.mcp.json` cannot run concurrently. W3 must
   answer how children share one frozen ruleset without N config swaps *before*
   the engine is built, or it inherits the serialization.
3. **F6/F7 (no workflow report, no shared ceiling)** are exactly the
   engine-level concerns W3 already lists as deliverables — confirmed as real,
   not speculative.

**Recommendation: proceed to W3, but treat F2 as a design-gate item** — resolve
the per-child config-isolation / shared-ruleset question in the W3 design
before implementation, because it determines whether the engine actually
delivers parallel fan-out or silently serializes like the interim path. F5, F6,
F7 are already answered by the W3 spec; F1, F3 are W2 polish worth doing
independently.

**What a second week of real use would add:** this week exercised
*read-only audit/review* fan-outs — the safest shape. A second week should
stress (a) a task where a child legitimately *fails* mid-map (does the
reduce degrade gracefully, or does one bad child poison the synthesis?);
(b) a longer map (10-20 children) to see whether the serialization in F2
becomes a practical blocker rather than a 47-min annoyance; (c) at least one
task that is *not* pure repo-audit — e.g. a cross-repo or web-research fan-out —
to check the trust-cliff (F8) and env story hold outside this one trusted
project; and (d) a run under an actual machine-policy ceiling to see whether the
absence of a shared budget (F7) ever produces a runaway before W3 exists. None
of these are blockers for starting W3; they would sharpen the ceiling and
failure-handling parts of its design. One week already clears the §9.4 bar.
