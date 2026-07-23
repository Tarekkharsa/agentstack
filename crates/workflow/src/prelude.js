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
