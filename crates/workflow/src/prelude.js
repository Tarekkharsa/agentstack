// Workflow prelude. Installed as a host-parsed Source BEFORE any untrusted
// script runs. Four phases, order load-bearing:
//   (1) determinism poisoning  — remove ambient nondeterminism, hardened so a
//       script cannot restore it (non-configurable, non-writable descriptors);
//   (2) orchestration helpers  — parallel / pipeline;
//   (3) split and group        — shard / partition;
//   (4) named algorithms       — mapReduce / reduceByKey / combine / verify /
//       keepUnrefuted, composed from (2) and (3) and nothing else.
// There is no runtime string compilation here; eval/Function(string) are denied
// by the host compile-strings hook, and this file is a trusted pre-parsed
// Source, so the denial does not block it.
(() => {
  "use strict";

  const denied = (name) =>
    function () {
      throw new Error(name + " is disabled in workflows (nondeterministic)");
    };

  // --- Phase 1: determinism poisoning -------------------------------------
  const OriginalDate = Date;

  // Harden Date.now first so its descriptor cannot be re-derived.
  Object.defineProperty(OriginalDate, "now", {
    value: denied("Date.now"),
    writable: false,
    configurable: false,
  });

  // Argless `new Date()` reads the clock; explicit-argument construction stays.
  const SafeDate = new Proxy(OriginalDate, {
    construct(target, callArgs, newTarget) {
      if (callArgs.length === 0) {
        throw new Error("argless new Date() is disabled in workflows");
      }
      return Reflect.construct(target, callArgs, newTarget);
    },
    apply() {
      throw new Error("Date() as a function is disabled in workflows");
    },
  });

  Object.defineProperty(globalThis, "Date", {
    value: SafeDate,
    writable: false,
    configurable: false,
  });

  // Rewire the prototype's constructor so `({}).constructor`-style walks land
  // on the proxy, not the raw Date.
  Object.defineProperty(OriginalDate.prototype, "constructor", {
    value: SafeDate,
    writable: false,
    configurable: false,
  });

  Object.defineProperty(Math, "random", {
    value: denied("Math.random"),
    writable: false,
    configurable: false,
  });
  Object.freeze(Math);

  // WeakRef.deref() (and FinalizationRegistry callbacks) observe the GC
  // schedule: the same script and inputs could return different results
  // depending on whether a collection ran between agent() slices — breaking
  // the determinism the resume journal replays against (§9.3 review
  // follow-up, 2026-07-23). Poisoned like Date.now, hardened the same way.
  for (const name of ["WeakRef", "FinalizationRegistry"]) {
    if (name in globalThis) {
      Object.defineProperty(globalThis, name, {
        value: denied(name),
        writable: false,
        configurable: false,
      });
    }
  }

  // Locale-sensitive APIs are host-varying, and that is fatal for the resume
  // journal specifically: the journal replays RESULTS, never DECISIONS. On
  // resume the script re-executes its own control flow and is only handed the
  // recorded outputs of steps it already ran, so every branch, sort order and
  // string it derives must come out identical on the machine that resumes.
  // A locale-aware comparison or format leaks the host's ICU data / default
  // locale into exactly that derivation: `localeCompare` can order two claims
  // differently on another machine, so a `sort()` reorders, so a different
  // item reaches step N, so the recorded output for step N is replayed onto
  // the wrong input — a silent divergence, not a refusal. Poisoned like
  // Date.now and hardened the same way.
  //
  // "Absent is as safe as denied": `boa_engine`'s default features are
  // `float16` + `xsum` — neither `intl` nor `experimental` — so `Intl` is not
  // built into this context at all and the string/date methods below are the
  // locale-INDEPENDENT fallbacks. Denying them anyway costs nothing and means
  // enabling `intl` later cannot silently reopen the hole.
  const localeSensitive = [
    [
      String.prototype,
      "String.prototype",
      ["localeCompare", "toLocaleLowerCase", "toLocaleUpperCase", "normalize"],
    ],
    [Number.prototype, "Number.prototype", ["toLocaleString"]],
    [
      OriginalDate.prototype,
      "Date.prototype",
      ["toLocaleString", "toLocaleDateString", "toLocaleTimeString"],
    ],
    [Array.prototype, "Array.prototype", ["toLocaleString"]],
  ];
  for (const [proto, label, names] of localeSensitive) {
    for (const name of names) {
      if (name in proto) {
        Object.defineProperty(proto, name, {
          value: denied(label + "." + name),
          writable: false,
          configurable: false,
        });
      }
    }
  }
  if ("Intl" in globalThis) {
    Object.defineProperty(globalThis, "Intl", {
      value: denied("Intl"),
      writable: false,
      configurable: false,
    });
  }

  // --- Phase 2: orchestration helpers -------------------------------------
  // AL3: parallel never rejects. A throwing thunk resolves that slot to null
  // (Claude Code null-on-failure, the same rule as a failed child run), so one
  // bad worker cannot throw an uncatchable rejection into the workflow.
  Object.defineProperty(globalThis, "parallel", {
    value: async (thunks) =>
      Promise.all(
        thunks.map(async (thunk) => {
          try {
            return await thunk();
          } catch (e) {
            return null;
          }
        }),
      ),
    writable: false,
    configurable: false,
  });

  // --- Phase 3: split and group helpers ----------------------------------
  // Plain JS over already-collected values: no agent spawn, no tokens, no
  // host call. They exist because the alternative — every script hand-rolling
  // its own chunking — is where off-by-one bugs and accidental unbounded
  // fan-out come from, and because `partition` is what makes a MULTI-reducer
  // workflow expressible at all (one reduce step cannot hold 500 mappers'
  // output in a single context window).
  //
  // Determinism is load-bearing (§3 rule, and Stage F journal replay depends
  // on it): both are pure functions of their inputs, key order is insertion
  // order, and neither reads a clock or a random source. `partition` hashes
  // the key STRING with a fixed FNV-1a — not `Math.random`, not object
  // identity, not iteration order of a Set — so the same items land in the
  // same shard on a replay, on another machine, and in another process.

  // shard(items, {per}) -> array of arrays, each at most `per` long.
  // The tail shard is short rather than padded; `per` below 1 is clamped so a
  // typo cannot produce an infinite loop.
  Object.defineProperty(globalThis, "shard", {
    value: (items, opts) => {
      const list = Array.from(items || []);
      const per = Math.max(1, Math.floor((opts && opts.per) || 1));
      const out = [];
      for (let i = 0; i < list.length; i += per) out.push(list.slice(i, i + per));
      return out;
    },
    writable: false,
    configurable: false,
  });

  // partition(items, r, keyFn) -> exactly `r` buckets (possibly empty), each
  // item placed by a stable hash of String(keyFn(item)).
  //
  // Exactly `r` buckets, including empty ones, on purpose: the caller maps
  // buckets to reducer agents, and a bucket count that silently varied with
  // the data would make the agent count — and therefore the ceiling
  // arithmetic — data-dependent.
  //
  // Same key => same bucket, which is the property a reducer relies on (all
  // findings for one file reach one reducer). It is NOT a balanced split:
  // skewed keys make skewed buckets, exactly as in Hadoop, and that is the
  // caller's problem to solve with a better key.
  Object.defineProperty(globalThis, "partition", {
    value: (items, r, keyFn) => {
      const list = Array.from(items || []);
      const count = Math.max(1, Math.floor(r || 1));
      const buckets = [];
      for (let i = 0; i < count; i++) buckets.push([]);
      for (let i = 0; i < list.length; i++) {
        const key = keyFn ? String(keyFn(list[i], i)) : String(i);
        // FNV-1a over UTF-16 code units, kept in 32-bit range with Math.imul
        // so the result is identical everywhere rather than drifting into
        // float territory for long keys.
        let hash = 2166136261;
        for (let c = 0; c < key.length; c++) {
          hash ^= key.charCodeAt(c);
          hash = Math.imul(hash, 16777619);
        }
        buckets[(hash >>> 0) % count].push(list[i]);
      }
      return buckets;
    },
    writable: false,
    configurable: false,
  });

  // AL2: pipeline is PER-ITEM (Claude Code semantics), not a waterfall over the
  // whole array. Each item runs through all stages independently, with no
  // barrier between stages; a stage callback receives (prevResult, originalItem,
  // index). A stage that throws drops that one item to null and skips its
  // remaining stages, leaving the other items unaffected.
  Object.defineProperty(globalThis, "pipeline", {
    value: async (items, ...stages) =>
      Promise.all(
        items.map(async (item, index) => {
          let value = item;
          for (const stage of stages) {
            try {
              value = await stage(value, item, index);
            } catch (e) {
              return null;
            }
          }
          return value;
        }),
      ),
    writable: false,
    configurable: false,
  });

  // --- Phase 4: named algorithm helpers -----------------------------------
  // Why these are safe to add at all: **not one of them calls `agent()`**.
  // Each is a pure composition of the helpers installed above (`parallel`,
  // `pipeline`, `shard`, `partition`) plus callbacks the CALLER supplies. The
  // only way an agent run happens is the caller's own callback invoking
  // `agent()`, which goes through the host bridge exactly as a hand-written
  // script's call does — so the bridge's `role ∈ meta.roles` check and the
  // `max_agents` ceiling remain the sole authority path, and naming an
  // algorithm here can never widen fan-out or bypass a refusal. If one of
  // these ever spawned on its own behalf, that argument would collapse.
  //
  // House rules, same as Phase 2/3: never throw (a failure becomes `null`,
  // mirroring `parallel`/`pipeline`), deterministic (no clock, no random, no
  // iteration order beyond insertion order), and total under junk arguments
  // (a missing callback, a non-array, a non-positive count) — clamped the way
  // `shard`/`partition` already clamp rather than throwing at the caller.
  //
  // Captured once, from the frozen globals installed above, so a helper can
  // never be re-pointed at something a later script installed.
  const parallelFn = globalThis.parallel;
  const pipelineFn = globalThis.pipeline;
  const shardFn = globalThis.shard;
  const partitionFn = globalThis.partition;

  // Run `thunks` through `parallel`, but treat a non-callable callback as a
  // failure of THAT slot (null) instead of a thrown TypeError at the caller.
  // Every reducer-shaped helper below funnels through this one place.
  const runBuckets = (buckets, fn) =>
    parallelFn(
      buckets.map((bucket, index) => async () => {
        // An empty bucket does NOT spend an agent: the callback is never
        // invoked and the slot is `null`. `partition` returns exactly `r`
        // buckets so the reducer COUNT stays fixed and data-independent, but
        // the reducer CALLS are only made for buckets that hold something —
        // a workflow whose data happens to leave a bucket empty pays nothing
        // for it, and the result array still has one slot per bucket.
        if (bucket.length === 0) return null;
        if (typeof fn !== "function") return null;
        return await fn(bucket, index);
      }),
    );

  // mapReduce(items, { map, reduce, partitions }) — the canonical
  // map -> shuffle -> reduce shape, spelled once so scripts stop re-deriving
  // it. `map` runs per item through `pipeline` (so one bad item becomes null
  // rather than failing the batch), nulls are dropped, the survivors are
  // shuffled into `partitions` buckets by `partition`, and each non-empty
  // bucket is reduced under `parallel`.
  //
  // Returns one slot per bucket, in bucket order — `partitions` results, of
  // which the empty buckets' slots are `null` (see `runBuckets`).
  Object.defineProperty(globalThis, "mapReduce", {
    value: async (items, opts) => {
      const o = opts || {};
      const list = Array.from(items || []);
      const count = Math.max(1, Math.floor(o.partitions || 1));
      // No key function: `partition` falls back to the item's index, which is
      // the right default for a plain map/reduce (no grouping requested).
      // Use `reduceByKey` when the reducer must see all of one key's items.
      const mapped =
        typeof o.map === "function" ? await pipelineFn(list, o.map) : [];
      const kept = mapped.filter((v) => v !== null && v !== undefined);
      return await runBuckets(partitionFn(kept, count), o.reduce);
    },
    writable: false,
    configurable: false,
  });

  // reduceByKey(items, r, keyFn, reduceFn) — `partition` then `parallel`, for
  // the case `mapReduce`'s index shuffle is wrong: every item sharing a key
  // lands in one bucket, so a reducer sees all of its key's items. Exactly
  // `r` result slots; empty buckets spend no agent.
  Object.defineProperty(globalThis, "reduceByKey", {
    value: async (items, r, keyFn, reduceFn) =>
      await runBuckets(partitionFn(items, r, keyFn), reduceFn),
    writable: false,
    configurable: false,
  });

  // combine(items, per, combineFn) — Hadoop's combiner. `shard` into chunks of
  // at most `per`, then run `combineFn(chunk, i)` over each under `parallel`.
  //
  // What it buys: a combiner cuts what the reduce stage has to READ. Hadoop's
  // cuts shuffle bytes; here the scarce resource is context, so it cuts reduce
  // TOKENS — pre-summarizing 200 findings into 20 chunk summaries means the
  // reducer's prompt holds 20 things, not 200. It is not a cheaper reduce, it
  // is a smaller one.
  Object.defineProperty(globalThis, "combine", {
    value: async (items, per, combineFn) =>
      await runBuckets(shardFn(items, { per }), combineFn),
    writable: false,
    configurable: false,
  });

  // verify(claims, refute) — the validation reducer as a first-class shape:
  // run an independent refuter over each claim under `parallel` and return
  // `{ claim, verdict }` in CLAIM order, one row per claim.
  //
  // `verdict` is `null` when the refuter failed (threw, or was never given) —
  // `parallel`'s null-on-failure rule, unchanged. Pairing the verdict back to
  // its claim is the whole point: an array of verdicts alone is one `filter`
  // away from being silently misaligned with the claims it judges.
  Object.defineProperty(globalThis, "verify", {
    value: async (claims, refute) => {
      const list = Array.from(claims || []);
      const verdicts = await parallelFn(
        list.map((claim, index) => async () => {
          if (typeof refute !== "function") return null;
          return await refute(claim, index);
        }),
      );
      return list.map((claim, index) => {
        const verdict = verdicts[index];
        return { claim, verdict: verdict === undefined ? null : verdict };
      });
    },
    writable: false,
    configurable: false,
  });

  // keepUnrefuted(claims, verdicts, isRefuted) — pure: no await, no agent, no
  // host call. `verdicts` may be either what `verify` returns (rows carrying a
  // `verdict`) or a bare array of verdict values aligned by index.
  //
  // ⚠ THE DEFAULT PREDICATE IS A TEXT HEURISTIC OVER UNTRUSTED MODEL OUTPUT,
  // NOT A TRUST BOUNDARY. It greps the stringified verdict for "refuted". A
  // refuter that phrases its finding differently ("this claim is false", "I
  // could not confirm") reads as unrefuted; a claim whose own text contains
  // the word reads as refuted; and a prompt-injected refuter can say whatever
  // it likes. This is the same honesty the docs' schema section states about
  // validated results: it constrains SHAPE, not content. Pass your own
  // `isRefuted(verdict, claim, index)` — ideally over a schema-validated
  // verdict field — whenever the answer matters.
  //
  // A `null` verdict (the refuter died) is deliberately NOT treated as
  // refuted. Failing closed here would mean silently DROPPING claims whenever
  // a child run failed — turning an infrastructure failure into a quiet
  // content deletion the invoker never sees. The claim survives, unjudged,
  // and it is the script's job to notice `verdict === null` if that matters.
  const defaultIsRefuted = (verdict) => {
    if (verdict === null || verdict === undefined) return false;
    // `toLowerCase` is the locale-INDEPENDENT case fold; the locale-aware
    // `toLocaleLowerCase` is poisoned in Phase 1 and must not be used here.
    const text = String(verdict).toLowerCase();
    // "not refuted" / "unrefuted" both contain "refuted", so they are ruled
    // out before the positive test — the conservative direction is "keep".
    if (text.indexOf("not refuted") !== -1) return false;
    if (text.indexOf("unrefuted") !== -1) return false;
    return text.indexOf("refuted") !== -1;
  };
  Object.defineProperty(globalThis, "keepUnrefuted", {
    value: (claims, verdicts, isRefuted) => {
      const list = Array.from(claims || []);
      const rows = Array.from(verdicts || []);
      const test = typeof isRefuted === "function" ? isRefuted : defaultIsRefuted;
      const kept = [];
      for (let i = 0; i < list.length; i++) {
        const row = rows[i];
        // Accept both shapes: a `verify` row, or a bare verdict value.
        const verdict =
          row !== null &&
          typeof row === "object" &&
          Object.prototype.hasOwnProperty.call(row, "verdict")
            ? row.verdict
            : row === undefined
              ? null
              : row;
        let refuted = false;
        try {
          refuted = Boolean(test(verdict, list[i], i));
        } catch (e) {
          // A throwing predicate keeps the claim, for the same reason a null
          // verdict does: dropping on failure is the destructive direction.
          refuted = false;
        }
        if (!refuted) kept.push(list[i]);
      }
      return kept;
    },
    writable: false,
    configurable: false,
  });
})();
