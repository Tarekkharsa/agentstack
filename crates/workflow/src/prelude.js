// Workflow prelude. Installed as a host-parsed Source BEFORE any untrusted
// script runs. Two phases, order load-bearing:
//   (1) determinism poisoning  — remove ambient nondeterminism, hardened so a
//       script cannot restore it (non-configurable, non-writable descriptors);
//   (2) orchestration helpers  — parallel / pipeline.
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
})();
