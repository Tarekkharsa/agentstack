# Scaling governed workflows

> **Status:** active technical design; Phases 0–3 and 5 shipped in v0.16.0
> (`ac76fc0`, `49c8f9c`); Phase 2b open, Phase 4 blocked — see the lane in
> `TODO.md`.<br/>
> **Capability contract:** [`workflows-capability.md`](workflows-capability.md)
> — authoring, authority, and evidence boundaries. This document does not
> restate them and never relaxes them.<br/>
> **Ordered work:** [`../../TODO.md`](../../TODO.md#workflow-scaling-lane)<br/>
> **Product status:** post-launch headline. The 2026-07-25 product review
> places the everyday loop (its F01–F06) ahead of this lane, and that
> sequencing stands.

## 0. Thesis

MapReduce is not fast because of its API. It is fast because of a
*constraint*: a map task is a pure function over a partition, so the framework
may re-run it, reorder it, place it anywhere, and race two copies. Agent tasks
are not pure — so the scaling program reduces to one idea: **buy back purity
where it can be proved, and spend the freedoms it unlocks.**

AgentStack is unusually well placed to do this. A profile already fences the
filesystem and egress dimensions, so "this task has no side effects" can be a
*checked property of the granted authority* rather than a promise in a
comment. Everything below follows from that.

## 1. What the analogy actually buys

Cargo-culting Hadoop produces a distributed system with none of its
advantages. What transfers:

| Hadoop concept | Transfers? | What it becomes here |
|---|---|---|
| HDFS as bulk storage | **No** | Results are kilobytes of text. Nothing is too large for one machine. |
| HDFS as an immutable content-addressed store | **Yes** | Prompts, results, and schemas referenced by `sha256`, never copied by value: dedup, bounded heap, cross-machine transfer, cheap resume, verifiable results. Child output identity is already recorded this way. |
| Data locality | **Inverted** | The scarce local resource is a trusted checkout with resolvable secrets and the right posture — *workspace affinity*. A scheduling constraint and a security control at once. |
| YARN RM / NodeManager | **Yes** | A scheduler plus worker daemons advertising capacity, harnesses, verified projects, postures. Admission stays central; enforcement stays local to each worker. |
| ApplicationMaster | **Already exists** | The drive loop is the per-run coordinator. |
| Combiner | **Yes** | Pre-reduce K map outputs before the reduce prompt. Hadoop's combiner cuts shuffle *bytes*; ours cuts reduce *tokens* — the same bottleneck. |
| Partitioner | **Yes** | R independent reducers instead of one, removing the single-context ceiling. |
| Sort/shuffle infrastructure | **Mostly no** | Grouping is plain JS in the script and costs zero tokens. |
| Speculative execution | **Conditionally** | Hadoop races copies because tasks are pure. An agent run may write files or call APIs. But a role whose profile denies writes *and* egress is provably effect-free — so speculation is permitted exactly where the granted authority proves it safe. |
| Task re-execution | **Same gate** | Bounded retry for effect-free roles; a mutating role's failure stays a human's re-run decision. |
| Job counters | **Yes** | Run-level aggregates in the recorder, rendered by `workflow report`. |
| JobTracker bookkeeping | **Already better** | Append-only, fail-closed, digest-bound, and doubling as the resume journal. |

## 2. Non-goals

- **No distributed filesystem.** Content addressing gives every property
  needed; replication and block placement give none.
- **No durability engine.** Unchanged from the capability contract:
  resume-by-journal stays the model.
- **No speculation or auto-retry for mutating roles.** Duplicated side effects
  are worse than a slow run. The gate is the profile and has no override.
- **No worker-supplied argv, paths, policy, or model selection.** That would be
  the second dispatch path rule 6 forbids.
- **No script-visible scheduling controls.** Concurrency, placement, retry, and
  speculation stay engine- and manifest-owned. A script that could negotiate
  its own placement is a grant-widening surface wearing a performance costume.
- **No central secret distribution.**

## 3. Measured baseline

`examples/workflow-scale` is the instrument: a latency-configurable mock
harness (latency derived deterministically from a checksum of the prompt, so
runs are reproducible), a width knob, and a concurrency knob. It costs zero
tokens and measures the *drive loop*, not model behaviour.

`efficiency` is `ideal_wall / actual_wall`, where `ideal_wall` is
`sum(this run's own child durations) / concurrency` — the best any scheduler
could do with exactly those children. There is deliberately no ungoverned
bookend arm; see the example's README for why one is not a fair denominator.

Before Phase 1 (2026-07-25, release binary at `7b9e101`):

| width | conc | wall_s | ideal_s | efficiency | straggler |
|------:|-----:|-------:|--------:|-----------:|----------:|
| 25  | 4  | 13.87 | 9.72  | 0.701 | 14.30 |
| 25  | 16 | 9.94  | 2.72  | 0.274 | 8.28  |
| 100 | 4  | 35.28 | 31.23 | 0.885 | 14.30 |
| 100 | 16 | 15.36 | 8.40  | 0.547 | 12.79 |

The finding that set the phase order: **the two bottlenecks are coupled.** At
concurrency 4 the pool is saturated, so the batch barrier costs little
(0.885). Raising the cap to 16 *exposed* the barrier rather than exploiting
the capacity (0.547) — the more concurrency the old drive loop was given, the
less of it it used. Neither fix was worth shipping alone.

## 4. Phases

### Phase 0 — measure — **done 2026-07-26**

`examples/workflow-scale/`: mock harness, `map+verify` pipeline parameterized
by width, `analyze.py`, and the sweep. Uses only the shipped report shape
(per-step and per-run `duration_ms` are already recorded), so it needed no new
CLI code.

### Phase 1 — continuous dispatch — **done 2026-07-26**

Replaced *fan out → join the whole batch → step* with a persistent worker pool
plus a completion channel: children stay in flight across `step()` calls, so a
later stage overlaps an earlier one. This makes the host honour the semantics
the script API already had — `pipeline()` is per-item and barrier-free by
contract, but a whole-batch join made item *i*'s second stage wait for the
slowest sibling.

Engine change: `StepOutcome::Awaiting`. Stepping with only *some* results
leaves the root promise legitimately pending with no new spawns, which the
engine previously reported as a stall. `Awaiting` is returned only while
requests are genuinely outstanding; a pending root with nothing outstanding
remains the internal failure it always was.

Preserved exactly:

- **Spawn evidence precedes launch.** Every `StepSpawned` for a batch is
  appended fail-closed before any of that batch is enqueued, so a batch's
  spawn events stay contiguous and all-before-execution — the shape Stage F's
  replay alignment reads.
- **Resume stays lockstep.** While a journal is active the drive waits for the
  whole batch; streaming begins on the first fully-live batch. None of Stage
  F's alignment reasoning is disturbed.
- **Park/swap children keep the project to themselves** via an exclusive lock
  rather than a serial phase. The locked layer's atomic sentinel remains the
  fail-closed backstop.
- **The cooperative wall check keeps its shipped semantics** — evaluated before
  a batch is dispatched, refusing the *next* batch rather than interrupting
  children in flight. Making it fire mid-flight is an enforcement-timing
  change and needs its own review; it is not part of a throughput phase.

Result — 22% at the widest cell, and the efficiency trend reversed:

| width | conc | wall before | wall after | eff. before | eff. after |
|------:|-----:|------------:|-----------:|------------:|-----------:|
| 25  | 16 | 9.94  | 9.65  | 0.274 | 0.296 |
| 100 | 4  | 35.28 | 34.09 | 0.885 | 0.935 |
| 100 | 16 | 15.36 | 12.00 | 0.547 | 0.675 |

This is short of the ~1.75× first projected. The projection ignored the
**critical path**: `combine` cannot start until every `verify` finishes, and
no scheduler removes that. Re-running the widest cell with a *flat* latency
distribution settles where the rest of the gap lives — efficiency **0.902**.
The drive loop is now close to optimal, and the residual loss under a heavy
tail *is* the tail. That makes Phase 4 the measured next win rather than an
assumed one.

Also landed: `workflow list` marks roles whose harness takes no per-child MCP
config (`*` in the table, `serial_roles` in JSON). A wide `parallel()` over
such a role executes sequentially regardless of the concurrency cap, and that
is an authoring-time fact that previously only appeared in the run log.

Deferred from this phase, with reasons: a second `max_inflight` knob (children
are I/O-blocked, so one cap suffices — a second semaphore would be
configuration surface with no measured win), and re-baselining
`DEFAULT_MAX_CONCURRENT` off 4 (needs a real-model rate-limit datum, not a
mock).

### Phase 2 — structured results and a content-addressed store

Two halves of one idea: results become *typed* and *addressed* rather than
free text held in the JS heap.

#### 2a — schema-validated results — **done 2026-07-26**

`agent(prompt, { schema })`, harness-agnostic and engine-unchanged: `opts`
already rides the `SpawnRequest`, so the CLI reads the schema there, appends a
JSON output contract to the prompt, and resolves the promise with the value
extracted and validated from the child's stdout. Implementation lives in
`crates/cli/src/commands/workflow_schema.rs`.

Decisions worth keeping visible:

- **No automatic re-ask.** A CLI-side retry would spawn a child the engine
  never counted against `max_agents` — a ceiling bypass. Validation failure
  fails the step closed (`null`) and the script decides. Retry accounting
  belongs to Phase 4, alongside purity.
- **A bounded JSON Schema subset, not a conformant implementation** — `type`,
  `properties`, `required`, `items`, `enum`, `additionalProperties: false`.
  This avoids a new dependency, and unsupported keywords are documented as
  *ignored* so no author reads one as enforcement.
- **Extraction is deliberately tolerant.** Models wrap JSON in fences and
  prose regardless of the prompt; failing a governed child over backticks
  would be a self-inflicted reliability problem. Whole-string, then fenced
  block, then a string- and escape-aware balanced scan.
- **An unusable schema fails closed rather than being ignored.** Silently
  dropping an oversized or malformed schema would hand the script unvalidated
  model output while the author believes it was checked.
- **The same transform runs on the Stage F replay path.** `read_verified_result`
  returns raw stdout, so a replay that skipped validation would feed a JSON
  *string* where the original fed an *object* — a silent divergence with every
  digest still matching. Witnessed by
  `a_replayed_schema_step_feeds_the_same_structured_value`.
- **Taint sources now serialize non-string results.** Reading `as_str()` alone
  would have dropped every structured result from the influence evidence.

The honest boundary, restated because it is easy to lose: **a schema-validated
result is not a trusted result.** Validation constrains shape, never content.
A prompt-injected child can return perfectly schema-valid lies, and the §7
data-flow caveat is unchanged.

#### 2b — content-addressed artifact store — pending

- `~/.agentstack/artifacts/<sha256>`: an immutable store for prompts and
  results, promoting the already-recorded output digest to the primary handle.
- A total resident-result byte cap; past it `agent()` returns a frozen opaque
  `{ digest, bytes, preview }` handle. Bounded heap without changing the
  common case. (The posture label currently admits "no JS heap cap exists
  in-process"; this is what bounds it in practice.)

### Phase 3 — partitioner and combiner — **done 2026-07-26**

Pure orchestration primitives; no new host capability, no tokens, no host call.

- `shard(items, {per})` and `partition(items, r, keyFn)` in the prelude.
  `partition` returns **exactly `r` buckets, including empty ones**, so the
  reducer count — and therefore the ceiling arithmetic — is not
  data-dependent. Placement is a fixed FNV-1a over the key string, not
  `Math.random` and not iteration order, so the same items land in the same
  shard on a replay, on another machine, and in another process. It is *not* a
  balanced split: skewed keys make skewed buckets, exactly as in Hadoop, and
  the remedy is a better key.
- `agentstack workflow explain <name>` — static and read-only. It goes through
  the *same* admission choke point as `run`, because reading the script at all
  is what rule 3 forbids for an untrusted bundle; `workflow list` remains the
  refusal-free surface for whether an entry is admissible.

  It reports the effective ceiling chain, per-role scheduling (which roles are
  serial), and a count of `agent(` call **sites** — string- and comment-aware,
  so the word in a comment or a longer identifier does not inflate it. The
  render states in as many words that sites are not calls: one site inside a
  loop runs once per item, actual fan-out is data-dependent and undecidable
  here, and the enforced bound on total spawns is `max_agents`. A number that
  implied an analysis we did not perform would be worse than no number.

### Phase 4 — purity, retry, and straggler speculation — **surface landed, execution blocked**

The declaration surface is implemented and **fails closed**. The execution
half is blocked on a prerequisite that does not exist yet, and this section
records that rather than shipping the unsafe version.

`[workflows.<n>.scheduling.<role>]` accepts `effect_free`, `retry`, and
`speculative`. All three are currently **refused by validation**, each with
its reason:

> `effect_free = true` … nothing in the current authority model can verify:
> `[policy.filesystem]` is bundle-global rather than per-profile, its `write`
> scope is enforced only in sandbox mode, and workflow children run at the
> host tier.

**The finding.** The plan asserted that "a profile already fences the
filesystem and egress dimensions, so purity can be a checked property." That
is not true of the shipped model. A `Profile` fences `servers`, `skills`, and
`harness` — nothing else. `[policy.filesystem]` is bundle-global, its `write`
scope is enforced only under `--sandbox`, and workflow children run with
`sandbox: false, lockdown: false`. Deriving `effect_free` from anything
available today would be a claim the enforcement cannot back, which rule 8
exists to prevent; accepting it as an author's assertion would let a repo file
buy a scheduling freedom, which the thesis exists to prevent. So it is
refused, loudly, with the prerequisite named.

**What unblocks it**, in preference order:

1. Per-profile filesystem and egress dimensions, bound and enforced for a
   role's children — then `effect_free` is derivable and checkable, as the
   thesis intended. This is an authority-model change and needs its own
   review.
2. Or: run a role's children under a posture where egress and filesystem
   *are* enforced (`--sandbox`/`--lockdown`), and derive purity from the
   posture instead of the profile.

Until one of those lands, a failed step surfaces to the script as `null` and
the workflow decides — which is the honest behaviour, just not the fast one.
Note the consequence for §4's measurement: the straggler tail is the largest
remaining source of lost wall-clock, and it stays unaddressed.

### Phase 5 — the dispatcher seam, local implementation only — **done 2026-07-26**

`crates/cli/src/commands/workflow_dispatch.rs`. One implementation ships —
`LocalDispatcher`, which is exactly the shipped behaviour — and the persistent
pool routes through the trait, so it is load-bearing rather than decorative.

The seam exists now, with nothing behind it, so that the *shape of the
request* is designed before there is a network to carry it: a wire format
invented alongside its first remote consumer is how authority-bearing fields
get added by accident.

`TaskDescriptor` carries **digests and names, never authority**. There is
deliberately no field for argv, policy, secrets, filesystem paths, model
selection, or a harness binary. The absence is the contract, and it has its
own witness (`a_task_descriptor_carries_no_authority`) rather than only a
comment, because the failure mode is silent — a remote backend would simply
start honouring a field that appeared.

Acceptance was preservation, and it held: the entire existing witness suite
passes unchanged through the seam, and the width-100 bench is unchanged
(11.87 s vs 12.00 s).

Still open from this phase: cancellation propagation (the watchdog orphans
in-flight children) and splitting the report/list/runs renderers out of
`workflow.rs` (review F16).

### Phase 6 — distributed workers — **trigger NOT met; not built**

**Ship only once measurement shows a real workload where, after Phases 1–4,
the *host* is the binding constraint — not the API rate limit and not the
token budget.** If the constraint is credential-side, the cheaper answer is
multiple credentials on one machine and this phase should be declined.

**Status 2026-07-26: the trigger is not met, so nothing was built.** The
measurements say the binding constraint at width 100 is the straggler tail
(efficiency 0.673 with a heavy tail against 0.902 flat), and the fix for that
is Phase 4's backup tasks — which are themselves blocked on the purity
prerequisite above, not on any shortage of machines. Adding machines would not
move either number. Building it anyway would be exactly the "capability lane
without user evidence" the working rules forbid.

The five non-negotiable rules:

1. **The coordinator sends digests, not authority.** Workflow digest, role
   name, profile digest, lock digest, project identity, prompt reference.
   Never argv, policy documents, secrets, or paths.
2. **Each worker re-verifies independently.** Trust state, lock digests, and
   policy intersection computed locally against the worker's own machine
   ceiling; effective authority is `min(request, worker ceiling)`. A worker
   that cannot verify the pins refuses the task rather than asking the
   coordinator what to believe.
3. **Secrets never cross the wire.** Each worker resolves its own `${REF}`
   locally; a missing ref fails the task closed and names the ref.
4. **Evidence is written locally and joined by digest.** Each worker keeps its
   own recorder log and returns
   `{run_id, grant_digest, outcome, result_digest, posture}`. No centralized
   write path.
5. **The worker is its own enforcement boundary.** A compromised coordinator
   must not be able to make a worker exceed that worker's machine policy.

Distributed mode adds a network attack surface, a second trust root (worker
identity), and cross-machine clock skew in the evidence timeline. Each needs a
labelled sentence in the posture text.

## 5. Success criteria

| Metric | Baseline | Target after Phases 1–4 |
|---|---|---|
| Drive-loop efficiency, flat latency, width 100 | 0.547 (heavy tail) | **0.902 achieved** at flat; heavy-tail target ≥ 0.85 after backup tasks |
| Straggler amplification per stage | 8–14 | ≤ 1.2 after backup tasks |
| Governance overhead vs. a real bookend | ~0% at N=5 | ≤ 5% at N=500 |
| Peak coordinator RSS at width 450 | unbounded (uncapped JS heap) | bounded by the resident-result cap |
| Largest reduce prompt | linear in width | bounded by combiner ratio × partition count |
| Evidence completeness | complete, fail-closed | **unchanged**, including retries and speculations |

## 6. Review findings in this lane

From the 2026-07-25 product review, the two findings scoped to workflows:

- **F13 — the approved graph and the executed bytes are two independent
  gates.** A user who approved a clean-looking blueprint then meets a second
  consent prompt they will read as ceremony. Bind them: show the blueprint
  digest beside the script digest, and pull the `plan_digest` binding forward
  rather than shipping the double-gate.
- **F14 — compile-on-approve is six writes with no rollback.** Make it one
  recorded, undoable transaction through the existing restore ledger; on any
  step failure revert the manifest entry and the script and name the step that
  failed.

Both must land before the capability is offered to anyone but the maintainer.
