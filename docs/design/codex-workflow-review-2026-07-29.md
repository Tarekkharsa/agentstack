# Cross-model review of the workflow interpreter — 2026-07-29

> **Status:** closed review record (workflows promotion gate input).
> Reviewer: gpt-5.6 Sol (High) via Codex CLI, static, read-only, at
> commit `6beac55`. Findings are folded into TODO.md's "Experimental
> workflows" promotion checklist; severities are calibrated to
> promotion readiness, not remote exploitability.

Severity here reflects readiness for promotion, not remote-exploit severity.

## Findings

1. **BLOCKING — The watchdog can block indefinitely before force-exiting.**  
   [workflow.rs:1185](crates/cli/src/commands/workflow.rs:1185)  
   On timeout, the watchdog performs `eprintln!`, filesystem creation/append, PID-mutex acquisition, and child signalling before reaching `process::exit` at line 1231. Any of those may block—for example, a full undrained stderr pipe or stalled filesystem. A hostile non-yielding script can therefore survive the mechanism intended to guarantee termination. The kill record is also best-effort: if one of these operations blocks, neither exit nor a durable terminal record is guaranteed.

2. **BLOCKING — Untrusted JavaScript has no effective memory or aggregate-value bound.**  
   [lib.rs:37](crates/workflow/src/lib.rs:37), [lib.rs:645](crates/workflow/src/lib.rs:645), [bridge.rs:448](crates/workflow/src/bridge.rs:448)  
   The context has loop, recursion, and stack limits, but no heap quota. JSON conversion limits nesting depth only; arrays, objects, property counts, strings, prompts, and option payloads have no byte/node/breadth bounds. Prelude helpers such as `partition` can also allocate a script-selected number of arrays. A repository workflow can OOM the entire agentstack process before the watchdog records recoverable state. Boa’s inherited buffer ceiling is far too large to serve as process containment.

3. **HIGH — Result serialization is re-entrant after the final pending-request drain.**  
   [lib.rs:480](crates/workflow/src/lib.rs:480), [lib.rs:485](crates/workflow/src/lib.rs:485), [bridge.rs:495](crates/workflow/src/bridge.rs:495)  
   `drive` drains pending agent requests and then calls `settle_root`. Result conversion invokes array/property access, proxy traps, getters, and rejection `toString` while the active bridge state remains installed. A result such as `{ get x() { agent("late", {role:"r"}); return 1 } }` can enqueue a request after the drain, yet the run is returned and recorded as `Done`. This leaves engine-owned pending work with no `StepSpawned` evidence and violates both re-entrancy and terminal-state consistency.

4. **HIGH — There is no general instruction or script-slice budget.**  
   [lib.rs:113](crates/workflow/src/lib.rs:113), [lib.rs:662](crates/workflow/src/lib.rs:662)  
   The configured Boa limits count loops, recursion, and stack depth, not total instructions or work inside native built-ins. Expensive regex/string/collection operations and job-queue behavior can therefore run until the external watchdog kills the entire process instead of producing a bounded interpreter error. Combined with finding 1, the instruction-bound invariant does not hold.

5. **MEDIUM — `Atomics.isLockFree()` exposes host-dependent nondeterminism.**  
   [prelude.js:18](crates/workflow/src/prelude.js:18), [Cargo.toml:12](crates/workflow/Cargo.toml:12)  
   The prelude removes `Date`, `Math.random`, `WeakRef`, and `FinalizationRegistry`, but leaves the default `Atomics` global available. In Boa 0.21.1, `Atomics.isLockFree(1|2|8)` reflects the platform’s atomic implementation. A workflow can branch on this and issue different requests across architectures, breaking cross-host resume determinism. `Atomics.waitAsync` also provides another unmetered liveness path under the fixed clock.

6. **MEDIUM — Script-size enforcement exists only in the current CLI caller, not the crate boundary.**  
   [meta.rs:146](crates/workflow/src/meta.rs:146), [lib.rs:264](crates/workflow/src/lib.rs:264), [workflow.rs:1141](crates/cli/src/commands/workflow.rs:1141)  
   The CLI caps scripts at 1 MiB, but public workflow APIs accept arbitrary `&str`, wrap/duplicate it, and parse it without enforcing that limit. Today’s call sites are protected; future direct consumers are not. A security boundary this important should be enforced inside the crate or represented by a validated bounded input type.

## Invariant evidence

1. **Isolation mostly holds:** every executable context installs `IdleModuleLoader`; trusted prelude and untrusted script evaluation share that context, dynamic compilation is denied, and the crate has no filesystem/network/process dependencies. I found no alternate evaluation path bypassing the denying loader.

2. **Input safety partially holds:** the CLI caps script and argument files, host paths come from admitted workflow metadata, and prompts are passed as structured process arguments—not interpolated into shell commands. Memory, aggregate-value, and crate-level size bounds are missing.

3. **Determinism partially holds:** a fixed clock and UTC offset are installed, date/random/weak-reference facilities are poisoned, JSON preserves insertion order, and recorded errors use stable slugs rather than absolute paths. `Atomics.isLockFree` remains reachable.

4. **Bridge topology holds:** `bridge.rs` is the only installed JS-to-host effect surface; roles and agent budgets are rechecked, and progress events are bounded. Prompt/options bounds and serialization-phase re-entrancy do not hold.

5. **State protocol partially holds:** the watchdog is armed before execution, spawn intent is journaled before child launch, and recorder appends use a single `O_APPEND` write. Blocking watchdog work and post-drain re-entrancy prevent a hard consistency guarantee.

**Recommendation:** yes, it is reasonable to keep shipping this as an honestly labeled experimental capability, given the existing trust/admission model and absence of filesystem, network, environment, module-loading, shell, or host-path escape. It is not reasonable to promote it out of experimental status until the watchdog has a lock-free/no-I/O hard-exit path, JS memory is contained, terminal conversion cannot re-enter the bridge, and the deterministic global surface is closed.

This was a static review of commit `6beac5555f403fbd6b32581af9d5c4040ba5a25f`; the read-only environment prevented rerunning the test suite.
