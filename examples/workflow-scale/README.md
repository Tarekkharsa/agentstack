# workflow-scale — the drive loop's scaling bench

`workflow-acceptance` proves the composition *works* at width 5 against a real
model. This bundle exists to prove how the **drive loop behaves under fan-out**,
which is a different question and needs a different instrument: a
latency-controlled mock harness, a width knob, and a concurrency knob.

Phase 0 of the workflow scaling plan. It costs zero tokens.

## Running it

```bash
AGENTSTACK_BIN="$PWD/target/release/agentstack" ./examples/workflow-scale/bench.sh
```

```bash
WIDTHS="5 25 100" CONC="4 16 64" ./examples/workflow-scale/bench.sh
```

```bash
SCALE_FLAT=1 ./examples/workflow-scale/bench.sh
```

Everything runs in a fenced `AGENTSTACK_HOME` under `mktemp -d`, with the mock
harness shadowing `claude` on `PATH`. Nothing touches your real setup.

## What it measures, and why these metrics

| metric | definition | why |
|---|---|---|
| `wall_s` | the run's recorded `duration_ms` | what the user waits |
| `ideal_wall_s` | `sum(child duration_ms) / concurrency` | the throughput floor — the best *any* scheduler could do with exactly these children |
| `efficiency` | `ideal / wall` | **the number Phase 1 has to move.** Below ~0.8 the drive loop is leaving worker capacity idle |
| `straggler_ratio` | `max(child) / median(child)` | how heavy the tail is; with a per-stage barrier the tail is paid once *per stage* |

There is deliberately **no ungoverned bookend arm** here. An `xargs` arm at the
same width draws a different latency multiset than the workflow's own prompts
(the mock derives latency from a checksum of the prompt), so the comparison is
noise — an early draft of `bench.sh` reported *−41% overhead*, i.e. the bookend
was slower than the governed run. `ideal_wall` is a per-run, apples-to-apples
floor computed from that run's own children, so `efficiency` isolates
drive-loop cost with no second arm needed. Real-model governance overhead stays
`workflow-acceptance`'s job, where a genuine bookend exists.

The mock's latency is **deterministic in the prompt** (`cksum`-derived), so the
same width replays with the same per-shard latencies — which is what makes a
before/after comparison of the drive loop meaningful at all.

## Baseline — 2026-07-25, before Phase 1

Release binary at `7b9e101`, macOS, mock harness at the default heavy tail
(70% 200 ms · 25% 800 ms · 5% 4000 ms).

| width | conc | wall_s | ideal_s | efficiency | straggler | steps |
|------:|-----:|-------:|--------:|-----------:|----------:|------:|
| 5   | 4  | 2.03  | 1.03  | 0.507 | 3.04  | 12/12   |
| 5   | 16 | 1.75  | 0.26  | 0.149 | 2.90  | 12/12   |
| 25  | 4  | 13.87 | 9.72  | 0.701 | 14.30 | 55/55   |
| 25  | 16 | 9.94  | 2.72  | 0.274 | 8.28  | 55/55   |
| 100 | 4  | 35.28 | 31.23 | 0.885 | 14.30 | 214/214 |
| 100 | 16 | 15.36 | 8.40  | 0.547 | 12.79 | 214/214 |

### What the baseline shows

**The two bottlenecks are coupled, and fixing either one alone is nearly
worthless.** This is the single most useful thing Phase 0 established.

- At **concurrency 4** the worker pool is saturated, so the batch barrier costs
  almost nothing: efficiency is 0.885 at width 100. Removing the barrier here
  would buy ~12%.
- At **concurrency 16** efficiency *collapses* to 0.547 — raising the cap
  exposed the barrier rather than exploiting the capacity. Going 4 → 16 at
  width 100 improved wall by 2.3× while the floor improved by 3.7×; the missing
  1.4× is stages waiting on each other.
- The pattern is monotonic in the wrong direction: the more concurrency you
  give the current drive loop, the *less* of it it uses (0.885 → 0.547 at width
  100; 0.701 → 0.274 at width 25).

So Phase 1 must land the barrier fix **and** the concurrency raise together.
Shipping either alone would produce a small win and a misleading benchmark.

**The tail is the mechanism.** Straggler ratios of 8–14 mean one child in a
stage routinely takes an order of magnitude longer than the median. Under a
per-stage barrier that tail is paid once per stage; under continuous dispatch
it is paid once per run. This is also what makes Phase 4's backup tasks worth
building later.

**Small runs are pure critical path.** At width 5 / conc 16, wall is 1.75 s
against a 0.26 s floor — nothing is throughput-bound, and essentially all of it
is four stages queued end to end.

## After Phase 1 — continuous dispatch

Same machine, same mock settings, same deterministic per-shard latencies.

| width | conc | wall before | wall after | eff. before | eff. after |
|------:|-----:|------------:|-----------:|------------:|-----------:|
| 5   | 4  | 2.03  | **1.85**  | 0.507 | 0.574 |
| 5   | 16 | 1.75  | 1.86      | 0.149 | 0.151 |
| 25  | 4  | 13.87 | 14.34     | 0.701 | 0.710 |
| 25  | 16 | 9.94  | **9.65**  | 0.274 | 0.296 |
| 100 | 4  | 35.28 | **34.09** | 0.885 | 0.935 |
| 100 | 16 | 15.36 | **12.00** | 0.547 | 0.675 |

**22% at the widest cell** (15.36 s → 12.00 s), and the efficiency trend is now
in the right direction: giving the loop more concurrency finally buys more of
it. The narrow cells barely move, which is expected — they are critical-path
bound, not throughput bound.

This is well short of the ~1.75× this document projected before Phase 0 ran.
The projection was wrong because it compared against the throughput floor
alone and ignored the **critical path**: `combine` cannot start until every
`verify` finishes, and `reduce` cannot start until every `combine` does. Those
are barriers in the *algorithm*, and no scheduler can remove them.

### Where the remaining gap actually is — measured, not guessed

Re-running the widest cell with a **flat** latency distribution
(`SCALE_FLAT=1`, no straggler tail) isolates the two effects:

| width | conc | distribution | wall_s | ideal_s | efficiency | straggler |
|------:|-----:|--------------|-------:|--------:|-----------:|----------:|
| 100 | 16 | heavy tail | 12.00 | 8.10 | 0.675 | 13.53 |
| 100 | 16 | **flat**   | 5.59  | 5.04 | **0.902** | 1.59 |

At 0.902 the drive loop is close to optimal. **The residual inefficiency under
a heavy tail is the tail itself, not scheduling overhead** — which means the
next real win is Phase 4's backup tasks for effect-free roles, and it is now an
evidence-backed claim rather than an assumption.

One incidental number worth keeping: in flat mode the configured child latency
is 200 ms but the recorded median step is **376 ms**. The ~176 ms difference is
the fixed cost of a governed child — process spawn plus the full locked
admission path — measured per child at width 100.

## Contents

- `bundle/` — a trustable project: `[workflows.scale-bench]` pinned, four
  empty-surface role profiles, and the script under `.agentstack/workflows/`.
- `bundle/.agentstack/workflows/scale-bench.js` — `pipeline(map, verify)` then
  `parallel(combine)` then `reduce`. Two agent stages inside one `pipeline()`
  is the shape the host barrier penalises; the `parallel(combine)` stage is a
  barrier that is genuinely inherent to the algorithm, kept so the bench can
  tell the two apart.
- `mock/claude` — the latency-configurable harness.
- `analyze.py` — one report JSON in, one metrics row out.
- `bench.sh` — the sweep.
- `results.jsonl` — raw rows from the most recent local run (gitignored;
  regenerated on every sweep, so the tables above are the recorded evidence).

## Limits worth stating

- The mock is a `sleep`, so this measures **scheduling**, not model behaviour.
  It cannot tell you anything about output quality, retries, or token cost.
- `duration_ms` per step comes from the child's own recorded run, so it
  includes process spawn and the full locked-admission path — the floor is
  honest about what a governed child costs.
- Width is bounded at 450 by the manifest's `max_agents` validation range
  (`1..=1000`), since the shape spawns `2W + ceil(W/8) + 1`.
