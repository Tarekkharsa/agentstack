// The workflow scaling bench — Phase 0 of the scaling plan.
//
// Written against the shipped v1 engine API only (no `schema`, no `partition`,
// no `shard` — those are Phases 2 and 3). Determinism rule observed: no
// Date.now, no Math.random, no argless `new Date` anywhere.
//
// SHAPE, and why it is this shape:
//
//   stage 1+2   pipeline(shards, map, verify)   <- TWO agent stages in ONE
//                                                  pipeline. This is the shape
//                                                  the host-side batch barrier
//                                                  penalises: `pipeline()` is
//                                                  per-item and barrier-free by
//                                                  contract, but the drive loop
//                                                  joins the WHOLE batch before
//                                                  stepping the engine again,
//                                                  so item i's verify cannot
//                                                  start until EVERY map has
//                                                  returned. With a heavy-tailed
//                                                  latency distribution the
//                                                  straggler is therefore paid
//                                                  once per stage instead of
//                                                  once per run.
//   stage 3     parallel(combine groups)        <- a genuine barrier: a combiner
//                                                  needs its whole group. Kept
//                                                  so the bench distinguishes
//                                                  barriers that are inherent to
//                                                  the algorithm from barriers
//                                                  the host imposes.
//   stage 4     agent(reduce)                   <- the single synthesis step,
//                                                  whose prompt growth is what
//                                                  Phase 3 (partitioner) exists
//                                                  to bound.
//
// Total spawns for width W: W + W + ceil(W/8) + 1.

export const meta = {
  name: 'scale-bench',
  description: 'Pipelined map+verify, then combine and reduce — the fan-out shape, parameterized by width',
  phases: [{ title: 'Map+Verify' }, { title: 'Combine' }, { title: 'Reduce' }],
  roles: ['mapper', 'verifier', 'combiner', 'reducer'],
}

const GROUP = 8

// Width comes from the invoker (`--args-json '{"width":100}'`). Untrusted
// input: bounded and integer-checked here rather than trusted.
// 450 is the largest width whose spawn total (2W + ceil(W/8) + 1 = 958) fits
// the manifest's own 1..=1000 max_agents validation bound.
const width =
  args && Number.isInteger(args.width) && args.width > 0 && args.width <= 450
    ? args.width
    : 8

const shards = []
for (let i = 0; i < width; i++) shards.push(i)

// Padding so each prompt and result is of realistic size — a 3-byte result
// would skip the taint detector's 64-byte floor entirely and measure a path no
// real run takes.
const PAD = 'x'.repeat(160)

phase('Map+Verify')
const checked = await pipeline(
  shards,
  (shard) => agent(`map shard ${shard} of ${width}. ${PAD}`, {
    role: 'mapper',
    label: `map:${shard}`,
  }),
  (mapped, shard) =>
    agent(`verify shard ${shard}: ${String(mapped).slice(0, 120)} ${PAD}`, {
      role: 'verifier',
      label: `verify:${shard}`,
    }),
)

const survived = checked.filter(Boolean)
log(`map+verify: ${survived.length}/${width} survived`)

phase('Combine')
const groups = []
for (let i = 0; i < survived.length; i += GROUP) {
  groups.push(survived.slice(i, i + GROUP))
}
const combined = await parallel(
  groups.map((group, gi) => () =>
    agent(`combine group ${gi}: ${group.length} verified inputs. ${PAD}`, {
      role: 'combiner',
      label: `combine:${gi}`,
    }),
  ),
)

phase('Reduce')
const reduced = await agent(
  `reduce ${combined.filter(Boolean).length} combined groups into one stance. ${PAD}`,
  { role: 'reducer', label: 'reduce' },
)

return {
  width,
  survived: survived.length,
  groups: groups.length,
  combined: combined.filter(Boolean).length,
  reduced: typeof reduced === 'string' && reduced.length > 0,
  spawned: budget.spawned(),
  remaining: budget.remaining(),
}
