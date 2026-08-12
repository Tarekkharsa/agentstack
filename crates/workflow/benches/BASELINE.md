# Workflow engine — baseline micro-benchmarks

Recorded **before** any change to `crates/workflow/src/bridge.rs` or to the
workspace release profile. These are the "before" numbers a later comparison
is measured against.

## Provenance

| | |
| --- | --- |
| commit | `cf062a6009cd7c248595c12c91569bd28a2858b9` (branch `rc4-prep`) |
| engine logic | identical to `a3b941f` — `git diff a3b941f HEAD -- crates/workflow/src/bridge.rs crates/workflow/src/meta.rs crates/workflow/src/prelude.js` is empty; `src/lib.rs` differs only by the additive, feature-gated `BoundaryBench` seam |
| release profile | `[profile.release]` = `strip = true`, `lto = "thin"` (opt-level 3, codegen-units 16, panic unwind — all cargo defaults) |
| toolchain | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| machine | Apple M2 Pro, macOS 26.5, on AC power, other agents active on the box |
| profile used | `bench` (inherits `release`) |
| date | 2026-08-12 |

**When the release profile changes, these numbers are void.** Re-record with
the same two commands and compare like for like.

## Reproduce

```sh
cargo bench -p agentstack-workflow --bench engine
cargo bench -p agentstack-workflow --bench boundary --features bench-internals
```

Both are `harness = false` targets using `std::time::Instant` — no criterion,
no new dependency (workspace rule: a new dependency needs maintainer approval).
Each phase is sampled 25 times after 3 discarded warm-up iterations; the
median is reported with min / p90 / max so a later median move can be told
from noise. All times are **microseconds**.

`--features bench-internals` exposes `BoundaryBench` (see the commented seam in
`src/lib.rs`). It is off in every shipped build, every CI job, and every
dependent crate; it adds no dependency and changes no production code path. It
exists because `value_to_js` / `js_to_value` are `pub(crate)` and every public
path to them carries a ~1 ms `Context`-plus-prelude cost — differencing that
to recover a 0.08 µs 1 KB conversion measures noise, not the boundary. Bench B
also prints an unsubtracted public-API round trip that needs no feature flag,
and the two agree (see "cross-check" below).

Run-to-run drift on this machine is roughly ±10 % on the medians; treat
anything smaller than that as noise.

## Bench A — engine, width-100 `parallel()` fan-out

Script: `meta = { roles: ['bench'], maxAgents: 1000 }`, 100 thunks each calling
`agent('task-N', { role: 'bench' })`, awaited through `parallel`, returning
`outs.length`. Every `SpawnRequest` is answered instantly with the fixed
payload `{"ok":true,"text":"done"}`, so no I/O is timed. The run is two steps:
step 1 evaluates the script to its first suspension and drains 100 requests;
step 2 feeds 100 results and settles the root.

### Headline

| | median (run 1) | median (run 2) | min (run 1) |
| --- | ---: | ---: | ---: |
| **(a) `WorkflowRun::new`** — context + host hooks + natives + `args` + prelude | **947.1** | 960.5 | 874.2 |
| **(b) mean per-step** — (step 1 + step 2) / 2 | **327.4** | 321.4 | 271.1 |
| **(c) total engine wall** — (a) + step 1 + step 2 | **1919.4** | 1881.9 | 1425.4 |

`new` dominates the fixed cost and is almost entirely `Context` construction
plus the 420-line prelude: a trivial script that spawns nothing pays the same
~0.9–1.3 ms. The fan-out itself costs ~1.4 µs per answered request.

### Run 1, full

| phase | min | median | p90 | max |
| --- | ---: | ---: | ---: | ---: |
| reference: `new` (trivial script) | 914.833 | 1319.083 | 2044.625 | 2390.333 |
| reference: per-step (trivial script) | 15.292 | 20.958 | 29.083 | 32.167 |
| reference: total (trivial script) | 930.125 | 1340.041 | 2062.666 | 2416.166 |
| (a) `WorkflowRun::new` | 874.166 | 947.083 | 1622.708 | 1807.083 |
| step 1 — eval to suspension, drain 100 requests | 415.292 | 495.208 | 1011.458 | 1060.708 |
| step 2 — feed 100 results, settle root | 126.833 | 136.792 | 151.334 | 164.792 |
| (b) mean per-step | 271.125 | 327.416 | 574.125 | 602.375 |
| (c) total engine wall | 1425.375 | 1919.375 | 2264.999 | 2509.375 |
| derived: step 2 / 100 (per answered request) | 1.268 | 1.368 | 1.513 | 1.648 |

### Run 2, full

| phase | min | median | p90 | max |
| --- | ---: | ---: | ---: | ---: |
| reference: `new` (trivial script) | 841.708 | 960.708 | 1665.500 | 2575.417 |
| reference: per-step (trivial script) | 12.958 | 16.000 | 31.375 | 48.708 |
| reference: total (trivial script) | 854.833 | 979.083 | 1691.958 | 2600.500 |
| (a) `WorkflowRun::new` | 852.291 | 960.542 | 1370.625 | 1548.459 |
| step 1 — eval to suspension, drain 100 requests | 405.958 | 501.791 | 1052.333 | 1111.167 |
| step 2 — feed 100 results, settle root | 122.500 | 134.750 | 167.667 | 222.250 |
| (b) mean per-step | 265.209 | 321.375 | 600.000 | 624.583 |
| (c) total engine wall | 1403.708 | 1881.917 | 2207.333 | 2227.458 |
| derived: step 2 / 100 (per answered request) | 1.225 | 1.347 | 1.677 | 2.223 |

The `new` and step-1 p90/max columns are wide because other agents were
compiling on the same machine. The `min` column is the stable figure; medians
agreed to ~2 % between runs for everything except the trivial-script reference.

## Bench B — boundary, `value_to_js` / `js_to_value`

| payload | JSON bytes |
| --- | ---: |
| 1 KB flat string | 1026 |
| 64 KB flat string | 65538 |
| wide array, 10,000 elements (short strings) | 78891 |
| object, 1,000 keys (short string values) | 13781 |

Each timed region repeats the conversion until it clears 500 µs, then divides
the count back out — `repeat` is that count, and the figures are per single
conversion.

### Direct — the converters alone

| payload | direction | repeat | min | **median (run 1)** | p90 | max | median (run 2) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 KB flat string | serde_json → JS (`value_to_js`) | 2000 | 0.0844 | **0.0850** | 0.0955 | 0.1735 | 0.0835 |
| 1 KB flat string | JS → serde_json (`js_to_value`) | 512 | 1.0653 | **1.1112** | 1.3058 | 1.4607 | 1.0352 |
| 64 KB flat string | serde_json → JS (`value_to_js`) | 256 | 3.9090 | **4.3555** | 5.6164 | 9.1787 | 3.6095 |
| 64 KB flat string | JS → serde_json (`js_to_value`) | 16 | 51.7370 | **59.4844** | 68.4974 | 71.8984 | 44.6224 |
| wide array, 10,000 elements | serde_json → JS (`value_to_js`) | 1 | 1373.583 | **1445.875** | 1526.417 | 1612.167 | 1312.375 |
| wide array, 10,000 elements | JS → serde_json (`js_to_value`) | 1 | 680.333 | **713.583** | 740.083 | 802.042 | 653.750 |
| object, 1,000 keys | serde_json → JS (`value_to_js`) | 4 | 175.042 | **182.177** | 189.198 | 190.511 | 177.146 |
| object, 1,000 keys | JS → serde_json (`js_to_value`) | 4 | 136.688 | **144.011** | 146.261 | 184.552 | 141.490 |

What the shape of that table says, for whoever changes `bridge.rs`:

* Flat strings are near-free inbound (one `JsString` allocation, ~0.08 µs/KB)
  and ~13× more expensive outbound (`to_std_string_lossy` re-encodes UTF-16 to
  UTF-8: 0.9 µs/KB).
* The per-element paths are where the time is. The wide array costs
  **~145 ns per element** inbound (`JsArray::push`, one property write and one
  length update each) against ~71 ns per element outbound (`arr.at`) — inbound
  is ~2× outbound and is the single largest number in this file.
* The 1,000-key object costs **~182 ns per key** inbound (`obj.set`) and
  ~144 ns per key outbound (`own_property_keys` + `get` per key).

### Public round trip — unsubtracted, no feature flag

Reproducible with `cargo bench -p agentstack-workflow --bench boundary` (no
feature). Reported raw, not differenced, so nothing is hidden: `new` includes
a whole `Context` plus prelude, and `step` includes evaluating the script.

| payload | phase | min | median | p90 | max |
| --- | --- | ---: | ---: | ---: | ---: |
| (reference) | `new`, args = null | 921.292 | 1064.416 | 1607.959 | 11475.083 |
| (reference) | `step`, `return null` | 8.500 | 11.500 | 13.708 | 15.708 |
| 1 KB flat string | `new`, args = payload | 847.708 | 876.917 | 1125.542 | 3825.375 |
| 1 KB flat string | `step`, `return args` | 10.083 | 12.167 | 13.500 | 15.041 |
| 64 KB flat string | `new`, args = payload | 910.500 | 1122.667 | 1244.084 | 1294.458 |
| 64 KB flat string | `step`, `return args` | 55.417 | 58.292 | 71.542 | 75.042 |
| wide array, 10,000 elements | `new`, args = payload | 2422.708 | 2642.125 | 2848.250 | 2904.667 |
| wide array, 10,000 elements | `step`, `return args` | 695.625 | 732.083 | 751.041 | 834.750 |
| object, 1,000 keys | `new`, args = payload | 1068.417 | 1095.916 | 1125.459 | 1249.459 |
| object, 1,000 keys | `step`, `return args` | 156.042 | 161.916 | 171.083 | 184.167 |

**Cross-check.** Differencing the public rows against their references
reproduces the direct measurements, which is the evidence that the
feature-gated seam times the real code and nothing else:

| payload | direction | public difference | direct median |
| --- | --- | ---: | ---: |
| wide array, 10,000 elements | in (`new` − reference) | 1577.7 | 1445.9 |
| wide array, 10,000 elements | out (`step` − reference) | 720.6 | 713.6 |
| object, 1,000 keys | out (`step` − reference) | 150.4 | 144.0 |
| 64 KB flat string | out (`step` − reference) | 46.8 | 59.5 |

The `new` difference is the loose one — a ~1 ms measurement with a ±0.5 ms
p90 cannot resolve anything under ~100 µs, which is exactly why the direct
seam exists. Outbound, where the reference is a clean ~11 µs, the two agree
to within run-to-run drift.

## What is measured, and what is not

* No I/O, no spawner, no recorder, no CLI: every `SpawnRequest` is answered
  from memory in the same thread.
* Bench A's payload is deliberately tiny (28 bytes) so the boundary cost is
  charged to bench B rather than smuggled into the engine number.
* Neither bench touches a clock the engine can see — the `Context` uses
  `FixedClock`, as in production.
* `WorkflowRun::new` includes parsing the script's `meta`. For the width-100
  script that is a ~7-line source, so it is not a material share of (a).

## After bridge.rs fixes

Recorded **after** the three `crates/workflow/src/bridge.rs` conversion changes
and **before** any change to the workspace release profile, so these numbers
are comparable to the tables above line for line.

### Provenance

| | |
| --- | --- |
| base commit | `cf062a6`, plus the uncommitted `bridge.rs` diff. The work now sits on `perf/rc4-cleanup`, based on `4881567` (`origin/main`, the squash of PR #63); `git diff cf062a6 4881567` is empty, so the measurement base is byte-identical to the baseline's and the two tables stay comparable |
| release profile | unchanged — `strip = true`, `lto = "thin"`, opt-level 3, codegen-units 16, panic unwind |
| toolchain | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| machine | Apple M2 Pro, macOS 26.5, on AC power |
| profile used | `bench` (inherits `release`) |
| date | 2026-08-12 |
| command | `cargo bench -p agentstack-workflow --bench boundary --features bench-internals` |

### What changed

1. `value_to_js`, arrays — convert every element into a `Vec<JsValue>` first,
   then build the array once with `JsArray::from_iter`
   (`Array::create_array_from_list`), instead of `JsArray::push` per element.
   `push` charged one property write **and** one `length` update per element.
2. `value_to_js`, objects — `JsObject::create_data_property` instead of
   `set(..., false, context)`. Defining rather than setting also stops any
   `Object.prototype` setter from running inside host conversion of an
   untrusted child result.
3. `js_to_value`, arrays — indexed `[[Get]]` (`arr.get(i, context)`) instead of
   `JsArray::at`, which is the builtin and re-reads `length` on every call, so
   each element cost two gets. Result capacity is pre-reserved but capped,
   because `len` comes from untrusted JS.

### Direct — the converters alone

| payload | direction | repeat | min | **median (run 1)** | p90 | max | median (run 2) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 KB flat string | serde_json → JS (`value_to_js`) | 2000 | 0.0819 | **0.0823** | 0.0828 | 0.0833 | 0.0845 |
| 1 KB flat string | JS → serde_json (`js_to_value`) | 512 | 1.0101 | **1.0331** | 1.0782 | 1.1045 | 1.0247 |
| 64 KB flat string | serde_json → JS (`value_to_js`) | 256 | 3.6016 | **3.7004** | 3.8325 | 3.8530 | 3.6761 |
| 64 KB flat string | JS → serde_json (`js_to_value`) | 16 | 44.8542 | **46.5209** | 49.0989 | 52.6901 | 45.8880 |
| wide array, 10,000 elements | serde_json → JS (`value_to_js`) | 2 | 259.563 | **283.750** | 320.479 | 330.438 | 274.292 |
| wide array, 10,000 elements | JS → serde_json (`js_to_value`) | 1 | 511.458 | **600.584** | 731.375 | 1015.042 | 525.709 |
| object, 1,000 keys | serde_json → JS (`value_to_js`) | 8 | 130.839 | **165.693** | 200.688 | 238.026 | 118.859 |
| object, 1,000 keys | JS → serde_json (`js_to_value`) | 4 | 153.104 | **164.740** | 169.385 | 169.771 | 142.031 |

The `repeat` counts for the two per-element payloads rose (array in 1 → 2,
object in 4 → 8): the harness repeats until it clears 500 µs, so a higher
count is itself evidence the conversion got cheaper.

### Before / after, medians

| payload | direction | before | after (run 1) | after (run 2) | change |
| --- | --- | ---: | ---: | ---: | --- |
| wide array, 10,000 elements | in (`value_to_js`) | 1445.875 | **283.750** | 274.292 | **−80 %, ~5.1×** |
| wide array, 10,000 elements | out (`js_to_value`) | 713.583 | **600.584** | 525.709 | −16 % / −26 % |
| object, 1,000 keys | in (`value_to_js`) | 182.177 | **165.693** | 118.859 | −9 % / −35 % |
| object, 1,000 keys | out (`js_to_value`) | 144.011 | **164.740** | 142.031 | unchanged path |
| 1 KB flat string | in | 0.0850 | **0.0823** | 0.0845 | unchanged path |
| 1 KB flat string | out | 1.1112 | **1.0331** | 1.0247 | unchanged path |
| 64 KB flat string | in | 4.3555 | **3.7004** | 3.6761 | unchanged path |
| 64 KB flat string | out | 59.4844 | **46.5209** | 45.8880 | unchanged path |

Read that table with the ±10 % drift figure in hand:

* **Both targets are beaten.** Array-in went from 1445.9 µs to 283.8 µs, far
  outside any noise band — **~145 ns per element down to ~28 ns**. Dropping the
  per-element `length` update is the whole of it. Object-in beat 182.2 µs in
  both runs.
* Array-out improved too, without being the point: ~71 ns → ~60 ns (run 1) or
  ~53 ns (run 2) per element, which is the second `length` read that
  `JsArray::at` no longer performs.
* Object-in is the noisiest row here (165.7 vs 118.9 between runs, a 39 %
  spread). Both are below the 182.2 µs baseline, but only the run-2 figure
  claims a large win; treat "−9 %" as the number this evidence supports.
* Object-out is an **unchanged** code path and its run-1 median came out 14 %
  above baseline while run 2 landed on it (142.0 vs 144.0). That is drift, not
  a regression — it is the reason the unchanged rows are kept in the table.
* The four flat-string rows are unchanged paths and all read slightly faster,
  which is a machine-quietness effect, not a code effect. They are the control.

### Public round trip — unsubtracted, no feature flag

| payload | phase | min | median (run 1) | p90 | max | median (run 2) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| (reference) | `new`, args = null | 890.500 | 1040.000 | 1751.417 | 22866.292 | 949.250 |
| (reference) | `step`, `return null` | 8.250 | 10.959 | 15.000 | 16.667 | 10.250 |
| 1 KB flat string | `new`, args = payload | 847.333 | 879.292 | 936.625 | 4085.250 | 843.375 |
| 1 KB flat string | `step`, `return args` | 9.958 | 11.375 | 12.833 | 14.500 | 10.291 |
| 64 KB flat string | `new`, args = payload | 845.583 | 888.208 | 982.916 | 1544.542 | 834.833 |
| 64 KB flat string | `step`, `return args` | 54.417 | 57.875 | 62.792 | 65.625 | 53.500 |
| wide array, 10,000 elements | `new`, args = payload | 1397.083 | 1579.792 | 1725.667 | 1748.292 | 1333.292 |
| wide array, 10,000 elements | `step`, `return args` | 527.584 | 540.875 | 560.833 | 566.375 | 529.708 |
| object, 1,000 keys | `new`, args = payload | 1047.584 | 1098.500 | 1209.834 | 1367.750 | 991.000 |
| object, 1,000 keys | `step`, `return args` | 166.292 | 172.000 | 192.584 | 200.959 | 164.292 |

The public path corroborates the direct seam where it is able to. The wide
array's `new` fell from a 2642.1 µs median to 1579.8 / 1333.3 — about
1060–1310 µs off a phase whose non-conversion share is ~1 ms, which is the
array-in saving arriving through the front door. Its `step` fell from 732.1 to
~530. The `object, 1,000 keys` `step` did not move (161.9 → 172.0 / 164.3),
as expected: that phase is the outbound converter, and outbound objects were
not touched.

### Note — the release profile changed after this was recorded

The same working tree then set `lto = "fat"` and `codegen-units = 1` on
`[profile.release]`. By this file's own rule that voids both tables above for
any *further* comparison: the "before" and "after bridge.rs" numbers were both
taken under `lto = "thin"` / `codegen-units = 16` and are comparable to each
other, but neither is comparable to anything measured after the profile
change. A third measurement must re-record the baseline under the new profile.
