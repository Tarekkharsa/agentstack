# ENFORCEMENT.md claim audit — 2026-08-05 (item N2)

**This is a read, not a rewrite.** Nothing below has been changed in
`docs/ENFORCEMENT.md`. Every row is a claim for the maintainer to dispose of,
because most of them have two possible fixes and only one of them is prose:
either the sentence is narrowed to what the code does, or the code is
strengthened to what the sentence says. That choice is not a documentation
decision.

**Why the pass was needed.** `tools/check-enforcement-pairing.py` stops *new*
drift by comparing a diff against a diff. It cannot judge whether a sentence
written months ago still describes today's code, and it watches only
`crates/trust/`, `crates/policy/` and `crates/egress/` — while most enforcement
claims in the document cite `crates/cli` paths.

**Coverage.** About 190 claim checks, roughly 180 distinct claims, against a
1051-line document. Not exhaustive. The unverified list at the end is part of
the deliverable, not an apology for it.

**Calibration.** The Hooks sections (lines 211-232 and 638-678), corrected in
`ea2eee2`, were spot-checked as a control and hold.

## Ranked findings

Ordered by blast radius. A claim that says something is enforced when it is not
outranks a claim that undersells the code.

| # | Claim, and its line | What the code does | Verdict | Smallest fix |
|---|---|---|---|---|
| 1 | **580** "an untrusted or drifted project materializes no skill files: nothing enters an agent's context before the gate passes" | `use --write` has no trust check. `run` → `prepare` → `activate` gates on the lockfile only (`use_profile.rs:190`), and `ensure_activatable` lets `Verdict::Unpinned` through (`verify.rs:141`). `skills::materialize` runs on `args.write` alone (`use_profile.rs:1036`); trust state is read only to print a word (`use_profile.rs:230`). The real gates are `session start` (`session.rs:291`) and the gateway. | OVERSTATES | Say skill files are pin-gated at `use --write` and trust-gated at `session start` and the gateway — not at materialization. |
| 2 | **561** "an untrusted or drifted project renders no server config and spawns nothing" | `apply --write` blocks a server plan only on unresolved secrets, resolve failure or policy refusal (`apply.rs:850-880`); no `trust::check` in that arm. "Spawns nothing" is correct (`session.rs:291`, `mcp_server.rs:809`, `locked.rs:1596`). Only hooks and extensions refuse on trust. | OVERSTATES (first half) | "…spawns nothing; server *config* is still written by `apply --write`, so the enforcement point is launch, not render." |
| 3 | **560** "Editing a server's command line therefore re-gates trust review" | Not for an `owner`-tagged server: `refresh_owned_servers` takes the definition from the owning app's own config (`render/owned.rs:52-102`) and `apply --write` then re-pins trust automatically (`apply.rs:1316-1370`). `check_server_reproducibility` reads the raw manifest (`doctor.rs:3374`), so it never sees the refresh. | OVERSTATES | Add the `owner = <adapter>` exception, whose refresh re-pins trust instead of re-gating it. |
| 4 | **495** the content-pin family "only ever fires for a project that is already trusted" | The whole-bundle refusal runs only when `require_trust` is set, and only `from_frozen` sets it (`gateway.rs:632`). On the host and lease path an untrusted project still resolves and pin-verifies every server (`gateway.rs:737`) and can emit the pin refusal (`gateway.rs:864`). | OVERSTATES | Scope it to the sandboxed run; on the host gateway path, naming the manifest dir is the consent. |
| 5 | **980, 982** "An unsigned bundle and an invalid signature are both stated on the card and neither aborts" | True interactively. `receive --yes` aborts on both: `confirmed()` bails when `!provenance.verifies()` (`share.rs:500-513`), and the gate needs `Verified`, not merely recognized (`publisher.rs:185`). | OVERSTATES | Scope both sentences to the interactive path and state the headless refusal. |
| 6 | **403** write confinement covers "file-tool writes outside the workspace" | Confinement runs only for `GuardEvent::FileWrite`. Cursor's arm maps every path-bearing payload to `FileRead` (`guard.rs:887`), so no Cursor file write is confined; elsewhere the write class is a fixed tool-name list (`WRITERS`, `guard.rs:1040-1062`) and an unlisted write tool falls through to `FileRead` (`:1061`), which runs the deny-glob check only. | OVERSTATES | "— on the CLIs and tool names the guard recognizes as writes; an unrecognized write tool, and every Cursor file tool, gets the deny-glob check only." |
| 7 | **595** instructions are "content-pinned per fragment, **trust-gated**, compiled into managed regions" | The pre-compile gate is lock-drift only, and unpinned fragments pass (`apply.rs:476-500`). `render/instructions.rs` reads standing decisions (`:95`) but never calls `trust::check`. | OVERSTATES | Drop "trust-gated", or qualify: manifest bytes are trust-bound, compilation is pin-gated. |
| 8 | **360** "a denied or unresolvable `${REF}` blocks the write rather than emitting a literal placeholder" | True for denied refs, which have no escape hatch. An *unresolvable* ref writes the literal `${NAME}` under `--allow-unresolved` (`adapters/src/render.rs:394-401`, `apply.rs:850-853`). | OVERSTATES | Split the two cases; name the flag. |
| 9 | **519, 466** "the host path does record its own refusals — the host-path egress check and secret-scope denials" | Secret denials do record (`secret/mod.rs:198`). `apply`'s write-time egress refusal records nothing — it pushes a string and continues (`render/apply.rs:381-398`); no `seatbelt::` call exists under `crates/cli/src/render/`. The only recorded egress refusal is the gateway-build one (`gateway.rs:927`). | OVERSTATES | Scope to the gateway-build check; say `apply`'s own refusal is printed, not recorded. |
| 10 | **1039** the `0.0.0.0` wildcard bind applies "**only** as a fallback when a Linux host cannot bind that gateway" | Two more paths need no bind failure: native Linux with an unknown gateway binds the wildcard at once (`egress/src/execution_relay.rs:81`, `runtime/src/docker.rs:91-107`), as does any OS that is not linux/macos/windows (`execution_relay.rs:83`). The code admits the first case at `execution_relay.rs:57-59`. | OVERSTATES | "…whenever the narrow address is unknown or unbindable" — and name the three cases. |
| 11 | **942** "Deleting all three changes what the review can *show*, never what it *decides*" | Standing decisions gate delivery on five paths: skill materialization (`use_profile.rs:574,589`), the MCP skill index (`mcp_server.rs:2436`), `skill_load` (`mcp_server.rs:2802`), instruction compilation (`render/instructions.rs:115`) and the protected run (`locked.rs:323`). A keep-pinned skill is served **from** the snapshot store (`use_profile.rs:589` → `store.rs:867`), and a missing or unverifiable snapshot fails delivery closed. | OVERSTATES the inertness — it hides a real enforcement mechanism | State that the snapshot store and standing decisions are read at delivery and load time, and that a missing snapshot fails a keep-pinned item closed. |
| 12 | **693** extensions write "zero extension bytes" for an untrusted project | The machine manifest is exempt (`render/extensions.rs:269`). | **CLOSED 2026-08-05** | Already fixed in `ded058b`: the bullet now names both exemptions. |
| 13 | **409** "Denials are recorded to the audit log (`host-guard` entries in `calls.jsonl`)" | Only evaluated-rule denials record. The three fail-closed system refusals — config unreadable, policy unavailable, payload unreadable — call `finish(proto, &deny, None)` with no audit (`commands/guard.rs:78-112`, gate at `:236`). | OVERSTATES | "Rule denials are recorded…; the fail-closed system refusals below are not." |
| 14 | **1038** "one pre-created, **1 MiB-capped** result-file bind" | Nothing caps the guest write: a plain read-write bind, chmod 0666, no size limit (`execution.rs:391-395`, `:593-600`). The 1 MiB is a host-side read refusal (`execution.rs:651`, `executor/src/lib.rs:26`, `:277`). The 16 MiB tmpfs, by contrast, is a real kernel cap (`runtime/src/spec.rs:71`). | OVERSTATES | "…whose contents are refused above 1 MiB on read (the write itself is not kernel-capped)." |
| 15 | **732** "`budget` meters agent count and wall clock, which the engine observes" | The engine is clock-free — "`max_wall_seconds` is INERT here… wall enforcement lives in the CLI's drive loop" (`workflow/src/lib.rs:218-220`). Wall enforcement is real, in the drive loop and a watchdog (`workflow_replay.rs:337-345`). | OVERSTATES (wrong enforcer) | "…agent count in the engine, wall clock in the CLI's drive loop and watchdog." |
| 16 | **819** "**every mutation** of the machine trust store appends one identity-only line" | `set_decision` mutates and saves without any `record_mutation` (`trust/src/lib.rs:165-186`). Only the four digest-bearing paths log (`:414`, `:507`, `:544`). | OVERSTATES (mild) | "every mutation of a project's *trust grant*". |
| 17 | **462-472** the denial-family table; "The trust-at-dispatch row is the **sixth** family" | A seventh family ships: `Family::Fence` has `audit_tool() == "fence"`, is in `AUDIT_TOOLS` and is written to `calls.jsonl` (`seatbelt.rs:105-123`, `:172-180`; emitted at `mcp_server.rs:662-673`). The code's own comment calls it "the seventh family". | STALE / INCOMPLETE | Add the row `Toolset-fence refusal \| enforced \| yes \| calls.jsonl (tool: fence)`; change "sixth" to "seventh". |
| 18 | **625** "is recorded as **F20** in `TODO.md`" | `TODO.md` was re-seeded on 2026-08-02 (`78fe54c`) and holds no `F20`. The rest of the Settings section is correct. | STALE REFERENCE | Replace with the real item id, or "not currently on the queue". |
| 19 | **412** "Claude Desktop / Junie expose no hook surface at all" | `NO_HOOK_SURFACE` has three members — claude-desktop, junie and **kiro** — and kiro's reason differs: hooks nest inside per-agent config files, not wired yet (`commands/guard.rs:508-518`). | STALE / INCOMPLETE | Name Kiro and its different reason. |
| 20 | **435** "nothing outside the mounted workspace directory is visible" | The spec also binds agentstack's own rendered config files read-only from a run-scoped temp dir (`sandbox.rs:1094`, `:1119`, `:819`, `:841`). No user filesystem outside the workspace is mounted, so the intent holds and the absolute wording does not. | OVERSTATES (minor) | "…apart from the read-only config files agentstack itself renders and mounts." |
| 21 | **916** "the only way to obtain one is through the depositing function (`Store::pin` for skills, `Store::pin_instruction` for fragments)" | A third sibling exists: `Store::pin_server_definition`, read back through the same re-hash guard (`store.rs:233`, `:256-284`). | UNDERSTATES | Add it to the parenthesis. |

## Unverified — needs a closer read

These were reached but not settled. Listed so the coverage claim above stays honest.

1. Whether `image` and `up` materialize skills with no trust check. Both appear in the trust grep with a different shape — this widens or narrows finding 1.
2. Whether the host guard's own hook is also rendered inside the sandbox and lockdown containers. If it is, those Filesystem cells gain a cooperative layer the document does not state.
3. Whether the Codex, Gemini, Copilot and Antigravity harnesses emit write-tool names that `WRITERS` covers. This decides how wide finding 6 is.
4. Whether `SecretAccess` can miss refs that resolve after gateway construction; emission is construction-only.
5. Whether the egress sidecar enforces the separate proxy token for `tools_execute`. Only the guest side was confirmed.
6. Whether every `RunEvent` variant the executor emits stores digests only (line 1042).
7. Whether the `share` witness "compares both runs byte for byte" (line 980) exists under that description.
8. The intake provenance-clock claims (lines 857-874): git tracking, `trust.jsonl` timestamp.
9. Whether `crates/cli/src/runs.rs`, cited at line 522, is a live citation or vestigial.

## Two structural notes

**The CI gate cannot see most of this.** `check-enforcement-pairing.py` watches
`crates/trust/`, `crates/policy/` and `crates/egress/`. Findings 1, 2, 3, 4, 5,
6, 7, 9, 11, 13, 15 and 17 all live in `crates/cli` or `crates/runtime`, which
the gate never examines — while most enforcement claims in the document cite
`crates/cli` paths. Widening the gate is itself a decision: it would fire on a
large fraction of CLI commits.

**Command names.** The document names `agentstack guard install`, `agentstack
mcp`, `agentstack report run`, `agentstack image`, `agentstack share` and
`agentstack publisher trust`. All six moved behind `agentstack x` in queue item
10, whose own rule is that a surface never names a command a reader cannot
find. Doctor already prints `agentstack x guard install` (`doctor.rs:4316`).
Hygiene, not an enforcement claim. The lease registry (W4,
`lease_registry.rs`) is also new and unmentioned; the "process-scoped" claim at
line 150 stays true.
