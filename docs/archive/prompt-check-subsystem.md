# Prompt: add a `check` subsystem to agentstack (governed verification)

> **Status:** unadopted brief, archived 2026-08-12. Moved here from
> `plan/prompt-check-subsystem.md`; nothing in the tree referenced it and
> [`TODO.md`](../../TODO.md) never queued it. The subsystem it proposes does
> not exist — there is no `agentstack x check` — so read it as a proposal, not
> as a description of the binary. Its "known facts" have drifted too: the
> workspace is eleven crates now, not the ten it names (`crates/mcp` became
> the protocol boundary on 2026-08-11; see
> [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md)).

> Status: merged final (2026-08-05). Two-agent review applied. Blocked on the
> STOP-SHIP hooks gate in plan/work-plan.html — Phase 0 verifies that first.

You are working in the agentstack repository. Read CLAUDE.md, STRATEGY.md,
TODO.md, and plan/work-plan.html first. Known facts: 10-crate workspace
(core, trust, policy, adapters, recorder, runtime, egress, executor,
workflow, cli), clap derive, tokio present, an MCP server ships
(`agentstack mcp`, stdio) plus a single-dispatch gateway, and `agentstack
verify` is TAKEN (ed25519 lock verification) — this subsystem is `check`.

Goal: one language-agnostic way for agents and humans to confirm work is
correct — `agentstack x check` (promotion to the visible 17 only via the
visibility rule) — structured pass/fail, exposed through the EXISTING MCP
surface. No second server, no second execution path, no second UI.

Three phases. Stop at end of Phase 0 and Phase A for approval.

## Phase 0 — Admission (no design, no code)
Write docs/design/check-admission.md: does this serve the shape; smallest
useful outcome; measurable exit criteria; the P8 argument (the repo's own
verification debt on run/workflow is this subsystem's first customer).
Propose the TODO.md entry. Confirm the STOP-SHIP hooks-gate fix has landed;
if not, say so and STOP — it outranks this. STOP for explicit adoption.

## Phase A — Recon and plan (no code)
Write docs/design/check-plan.md (never repo root):
1. Two existing commands to mirror (file paths); how `agentstack x` grouping
   works.
2. Where `[check]` slots into the manifest load path — and state plainly:
   `[check.commands]` declares commands to run = an EXECUTABLE capability.
   Trusted surface: consent-digest covered, shown on the review card, inert
   until the gate succeeds, re-gated on any byte change. Full ceremony —
   never compressed. Trust gates the project; the ceremony byte-binds the
   config: even preset defaults like `cargo test` execute repository code
   (build.rs, proc macros, tests), so there is no default carve-out.
3. Seam reuse: execution through the executor/runtime seam (policy ceilings
   apply; the guard sees it) — no bare tokio::process; every run leaves
   recorder evidence (single execution memory, no new store); MCP = two
   tools on `agentstack mcp` + fixed names in ui_contract.rs — no rmcp.
   Plan the no-spawn structural witness here: inside crates/check/src,
   forbid std::process, tokio::process, Command::new (clippy
   disallowed-methods or source-grep test). The runner calls the executor
   API only; tests/ may use assert_cmd — driving the built binary IS the
   witness, not a bypass.
4. New deps need maintainer approval: list exactly (schemars, insta,
   command-group only if the executor seam lacks process-group kill) with
   one-line justifications.
5. Result schema as concrete Rust types; CLI surface; fixture test plan.
STOP for approval.

## Phase B — Implement
Scoped commits: schema+runner, presets, gate+CLI, MCP+contract,
fixtures+tests, docs.

Architecture (crates/check/):
- schema.rs — VerbResult/GateResult, serde + schemars; `rule` an enum;
  evidence truncated head+tail with marker; each VerbResult carries
  `evidence_ref` (recorder run id) — the join key to recorded evidence.
  Same types serve --json, recorder values, MCP schemas.
- runner.rs — thin wrapper over the executor seam: cwd, per-verb timeout
  (default 300s), streams capped 1 MB, env allowlist, process-group kill.
  Timeout/crash → rule "timeout"/"tool_crash", never a panic. Secrets via
  ${REF} only, after trust; unresolved fails closed.
- presets/ — trait { id, detect(marker files), default_commands(format,
  lint, typecheck, build, test), parse }. rust.rs (cargo fmt --check /
  clippy / build / test as subprocesses — never link cargo internals),
  typescript.rs (prefer package.json scripts; tsc --noEmit, eslint
  --format json), python.rs (ruff / mypy if present / pytest standard
  output — add no plugins to user projects), generic.rs (exit 0 = pass;
  nonzero = one violation, both streams as evidence). One registry; new
  language = one module + one line. Parsers never crash: fall back to
  exit-code semantics with raw output. Detection resolves upward; optional
  path arg. All project content is hostile input: parse defensively, never
  shell-interpolate, execute nothing during detection or init. No preset
  detected AND no [check] section → refuse with a finding that names the
  next step: `agentstack x check init`.
- gate.rs — ordered verbs, fail-fast, skipped reported as skipped.
- cli.rs / mcp.rs — adapters only; both call the same gate function.

Config (manifest, trust-gated per Phase A): [check] preset/order/fail_fast;
[check.commands] overrides; [check.timeouts]. Invalid config fails loudly
with a docs pointer.

CLI: `agentstack x check [path]` (gate; human summary; --json; exit 0 iff
pass) · `x check <verb> [path]` · `x check init [path]` — detects, PREVIEWS
the [check] section, writes only with --write, refuses overwrite without
--force. Match existing tone; findings name their next step.

MCP: check_project(path?) and check_verb(verb, path?) on the existing
server; schemas from the shared schemars types; doc comments tell agents
"on failure, fix the listed violations and call again". Fixed names in
ui_contract.rs; never a generic run_command.

Tests: tests/check_fixtures/ — known-good + known-bad per preset. Assert:
detection; good passes / bad fails with expected rule and non-empty
location; 1s-timeout fixture → timeout violation; --json validates against
schema; overrides beat defaults; the no-spawn structural witness; a witness
that an UNTRUSTED project's [check.commands] never executes; an
evidence_ref-resolves assertion per gate run. Skip execution tests
gracefully without toolchains; parser tests run everywhere on committed raw
samples via insta. Loop: `cargo check -p agentstack-check`; focused tests
only; before handoff `cargo check --workspace --all-targets`, `cargo fmt
--check`, relevant clippy. Never the full suite locally.

Docs: how-to "Check your agent's work" with a Limits section (what check
does NOT prove); CLI reference; one intro paragraph. Honesty register:
checking is not proving; a passing gate is not a security claim.

Acceptance:
1. Fresh Python fixture: `x check init --write && agentstack trust . &&
   x check` works zero-config with correct results.
2. Same for TS and Rust fixtures when toolchains exist.
3. A failing test yields location+evidence sufficient to fix without
   rerunning.
4. MCP check_project returns byte-identical JSON to `x check --json`.
5. Untrusted project: check refuses and names `agentstack trust .`.
6. Repo lint/fmt/clippy clean.
7. check-plan.md retained in docs/design/ as the decision record.

Flag for line-by-line review: the trust-surface change, the executor-seam
wiring, the ui-contract addition.
