# Boa embedding spike report

## Setup and version

**boa_engine 0.21.1 — exact locked crates.io release.** `cargo add boa_engine` selected
0.21.1 with its default `float16` and `xsum` features. `Cargo.lock` records crates.io
checksum `1521be326f8a5c8887e95d4ce7f002917a002a23f7b93b9a6a2bf50ed4157824`.
`Cargo.toml` contains an empty `[workspace]`, so this crate cannot be captured by a parent
workspace.

The sandbox could read but not populate `~/.cargo/registry`, so Cargo was given a local
`CARGO_HOME=.cargo-home`. All API conclusions below come from directly reading that fetched
0.21.1 source, especially `object/builtins/jspromise.rs`, `job.rs`, `context/mod.rs`,
`context/hooks.rs`, `context/time.rs`, `vm/runtime_limits.rs`, `vm/mod.rs`, and
`builtins/math/mod.rs`; no API was inferred from memory.

Test machine: macOS 26.5, arm64, rustc 1.96.0, cargo 1.96.0.

## 1. Promise bridge — PASS

The registered native `agent(prompt, opts)` converts the arguments, creates a genuinely
pending `JsPromise`, and moves the prompt, serialized options, and both resolver handles to
a Rust queue before returning the promise:

```rust
let (promise, resolvers) = JsPromise::new_pending(context);
SPAWN_QUEUE.with(|queue| {
    queue.borrow_mut().push(SpawnRequest {
        prompt,
        opts_json,
        resolvers,
    });
});
Ok(promise.into())
```

The event loop calls `Context::run_jobs()`, drains all currently pending host requests,
calls each stored `resolve`, runs jobs again, and inspects `JsPromise::state()` until the
root settles. The fulfilled root value is installed as a temporary global, evaluated through
`JSON.stringify`, and extracted as a Rust `String`.

Real `cargo run --release` output:

```text
=== 1. PROMISE BRIDGE ===
[1] root after eval: Pending
[1] drain batch #1: count=1 [alpha opts={"retries":0,"role":"reviewer"}]
[1] PASS root JSON in Rust: {"resumed":"result-for:alpha","final":"root-saw:result-for:alpha"}
```

This proves evaluation returned to Rust while `agent()` was pending, resolving the Rust-held
function resumed the suspended `await`, and the async IIFE's final object round-tripped.

## 2. Parallel, pipeline, and language coverage — PASS

The prelude is deliberately small:

```javascript
globalThis.parallel = async (thunks) => Promise.all(thunks.map((thunk) => thunk()));
globalThis.pipeline = async (items, ...stages) => {
    let value = items;
    for (const stage of stages) value = await stage(value);
    return value;
};
```

The canonical script maps three destructured inputs to async thunks, awaits `parallel`, uses a
plain-JS deterministic shuffle, then passes the result through `map`, `flatMap`, `filter`, and
`reduce`. It JSON round-trips the result and verifies the output shape in a `for..of` loop. A
deliberately invalid `JSON.parse` exercises destructuring in `catch`.

Real output:

```text
=== 2. PARALLEL / PIPELINE + LANGUAGE COVERAGE ===
[2] root after eval: Pending
[2] drain batch #1: count=3 [map:alpha opts={"governed":true,"index":0} | map:beta opts={"governed":true,"index":1} | map:gamma opts={"governed":true,"index":2}]
[2] PASS canonical script JSON: {"shapeOK":true,"caught":true,"sum":12,"names":["beta","gamma","alpha"],"firstStdout":"result-for:map:beta"}
[2] language feature failures: none
```

All three calls were pending before the first host drain, so a production host can start the
three governed child processes concurrently. No requested language feature failed: template
literals, arrows, destructuring, spread, `map`/`filter`/`flatMap`, JSON parse/stringify,
optional chaining, `for..of`, async/await, and try/catch all executed successfully.

## 3. Determinism poisoning — PASS for tested attacks, with a hardening caveat

The cheapest mechanism works: before untrusted code runs, a prelude replaces global `Date` with
a non-configurable proxy that rejects zero arguments, replaces `Date.now` and `Math.random` with
non-configurable throwing functions, rewires `Date.prototype.constructor` to the proxy, and
freezes `Math`. Explicitly constructed dates remain available. The Context also uses
`FixedClock(0)` as defense in depth.

Key poisoning code:

```javascript
const SafeDate = new Proxy(OriginalDate, {
    construct(target, callArgs, newTarget) {
        if (callArgs.length === 0) throw new Error('argless new Date() is disabled');
        return Reflect.construct(target, callArgs, newTarget);
    }
});
Object.defineProperty(OriginalDate, 'now', {
    value: denied('Date.now'), writable: false, configurable: false
});
Object.defineProperty(globalThis, 'Date', {
    value: SafeDate, writable: false, configurable: false
});
Object.defineProperty(Math, 'random', {
    value: denied('Math.random'), writable: false, configurable: false
});
Object.freeze(Math);
```

The 12 attempted bypasses were direct calls, argless construction, `Function('return this')`,
`({}).constructor.constructor`, `Reflect.get`, `Reflect.construct`, the Date prototype's
constructor, extracting the property descriptor's value, indirect `eval`, and delete/reassign.
Boa's default build has no `ShadowRealm`, so no fresh realm could be created.

Real output:

```text
=== 3. DETERMINISM POISONING ===
[3] PASS attack results: {"restored":[],"blockedCount":12,"realm":"ShadowRealm unavailable","explicitDateStillWorks":"1970-01-01T00:00:00.000Z","functionGlobalIsGlobal":true}
[3] host-level backstop: FixedClock(0); no HostHooks/intrinsics switch removes Date or Math.random
```

Fetched-source inspection found a host-level clock abstraction: `ContextBuilder::clock` can
install `FixedClock`, and Date reads `context.clock()`. That makes time deterministic but does not
make the API unavailable. There is no `HostHooks` method or ContextBuilder switch to omit Date or
Math.random, and 0.21.1 implements `Math.random` directly with `rand::random::<f64>()`. The host
could mutate intrinsic/global property descriptors through Rust rather than a JS prelude, but that
is the same descriptor-hardening strategy, not a lower-level capability switch. No tested script
restored the functions, but this is defense-in-depth evidence rather than a proof against every
future Boa language feature or engine bug.

## 4. Runaway scripts — PASS for host containment; FAIL for JS catchability

The spike configures the shipped `RuntimeLimits` API directly:

```rust
let limits = context.runtime_limits_mut();
limits.set_loop_iteration_limit(10_000);
limits.set_recursion_limit(32);
limits.set_stack_size_limit(10_240);
```

The 0.21.1 defaults are no loop limit (`u64::MAX`), recursion 512, stack size 10,240, and
backtrace limit 50. Both exceeded-limit errors below return as Rust `JsResult::Err`; the same
Context successfully evaluates more code afterward.

Real output:

```text
=== 4. RUNAWAY SCRIPTS ===
[4] configured limits: loop=10000 recursion=32 stack=10240
[4] PASS infinite loop host-caught after 0 ms: RuntimeLimit: Maximum loop iteration limit 10000 exceeded
    at <main> (unknown at :?:?) (JS catch bypassed)
[4] context after loop error: 42
[4] PASS deep recursion host-caught: RuntimeLimit: exceeded maximum number of recursive calls
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at recurse (unknown at :1:36)
    at <main> (unknown at :1:49)
[4] context after recursion error: 42
[4] JS catchability: RuntimeLimit is intentionally non-catchable by JS; Rust receives JsResult::Err
[4] instruction limit: available only with boa_engine feature `fuzz` (not enabled here)
[4] wall-clock: no interrupt/callback for synchronous eval; evaluate_async_with_budget can cooperatively yield for an external timeout
```

The test intentionally wrapped the infinite loop in JS `try/catch`; 0.21.1's
`JsNativeErrorKind::RuntimeLimit` is explicitly marked non-catchable, so the handler is bypassed.
Thus “catchable” is true at the Rust host boundary and false inside JavaScript. This is safer for
enforcement because a workflow cannot swallow the limit, but it does not satisfy a requirement for
a script-visible catch.

Instruction/time investigation:

- A per-bytecode instruction counter exists only behind Boa's `fuzz` feature. It is documented as
  fuzzing-only, configured with `ContextBuilder::instructions_remaining`, and its forced-exit error
  is also non-catchable. The spike retains the default features and does not treat this as a
  production limit API.
- `Script::evaluate_async_with_budget` is public and cooperatively yields after an approximate
  opcode-cost budget. An async host can race/poll it against a timer and discard the Context after
  cancellation. This is useful, but a long native/builtin operation can delay the next yield.
- There is no synchronous evaluation callback, atomic interrupt flag, deadline, or production
  instruction-fuel API in the fetched 0.21.1 source.

**Explicit wall-clock answer: NO, a true wall-clock interrupt of a single synchronous JS slice is
not available in boa_engine 0.21.1.** Synchronous `eval` has iteration/recursion/stack limits and
the host regains control between job batches. The separate async evaluator can cooperatively yield
often enough for an external timeout, but that requires using the async evaluation path and is not
an asynchronous preemption of `Context::eval`.

## 5. Panic containment — PASS

The native function simply panics. Evaluation and the mutable Context are wrapped as required:

```rust
let caught = panic::catch_unwind(AssertUnwindSafe(|| {
    context.eval(Source::from_bytes("panicNative()"))
}));
```

Real output:

```text
=== 5. PANIC CONTAINMENT ===
[5] panic hook observed: intentional native panic from proof function
[5] PASS catch_unwind caught: intentional native panic from proof function
[5] same Context reuse probe: returned 42 (still discard after panic)
[5] process continued; fresh Context result: 42
[5] UnwindSafe friction: Context required AssertUnwindSafe
```

The unwind was contained, reported, and the process continued. `Context` is not accepted as
`UnwindSafe`, hence `AssertUnwindSafe`. This specific Context happened to remain reusable after
the unwind, but that is not a contractual recovery guarantee; production should discard any
Context crossed by a panic.

## 6. Footprint — PASS, with dependency and unsafe-code approval notes

Commands used:

```sh
cargo tree -e normal --prefix none \
  | sed -E 's/ \(\*\)$//; s/ \(proc-macro\)//' \
  | sort -u | grep -v '^boa-spike ' | wc -l
cargo clean
/usr/bin/time -p cargo build --release
stat -f 'BINARY_BYTES=%z' target/release/boa-spike
```

`cargo tree` result: **127 distinct normal transitive package/version nodes**, excluding the
`boa-spike` root. This is deduplicated rather than the raw indented tree line count. For reference,
Cargo metadata contains 137 non-workspace packages when all dependency kinds and target-specific
packages are included.

Notable dependency groups in the normal graph:

- Boa split crates: `boa_engine`, `boa_parser`, `boa_ast`, `boa_gc`, `boa_interner`,
  `boa_string`, and `boa_macros`.
- Unicode/ICU: `icu_normalizer`, `icu_normalizer_data`, `icu_properties`,
  `icu_properties_data`, `icu_provider`, `icu_collections`, `icu_locale_core`, `zerovec`,
  `zerotrie`, `yoke`, `tinystr`, and related crates. The `intl` Boa feature is off, but Unicode
  normalization/property data is still in the normal graph.
- RegExp: `regress` (rather than the `regex` crate); its unpacked source is about 2.5 MiB.
- Async/concurrency: `futures-lite`, `futures-concurrency`, `futures-channel`, `dashmap`, and
  `parking_lot_core`.
- Numeric/data support: `num-bigint`, `rand`/`rand_chacha`, `serde`/`serde_json`, `time`,
  `hashbrown`, `indexmap`, `float16`, and `xsum`.

Real clean-build timing tail:

```text
   Compiling boa_engine v0.21.1
   Compiling boa-spike v0.1.0 (/private/tmp/claude-608725469/-Users-tarek-k-Documents-GitHub-agentstack/beb69084-fe8a-4c45-bfd8-9471d1f41d1d/scratchpad/boa-spike)
    Finished `release` profile [optimized] target(s) in 49.88s
real 49.93
user 187.99
sys 15.24
```

Binary evidence:

```text
BINARY_BYTES=10022368
-rwxr-xr-x@ 1 tarek.k  wheel   9.6M Jul 22 07:40 target/release/boa-spike
target/release/boa-spike: Mach-O 64-bit executable arm64
265532f043413ddd9f7a5cd81f6586c1318e3d70c0eba8b968f42506b6fcb059  target/release/boa-spike
```

`boa_engine` is pure Rust in the sense that it is a Rust implementation rather than bindings to
a C/C++ JavaScript engine, but **it does not forbid unsafe code and it uses unsafe code**. Its
`lib.rs` has no `forbid(unsafe_code)` or `deny(unsafe_code)`. Direct examples in 0.21.1 include
`Box::from_raw` in `host_defined.rs`, pointer/tag operations and unsafe `Send`/`Sync` impls in
`symbol.rs`, and unsafe closure storage/interop in `native_function`. A lexical scan found 137
`unsafe {` tokens under the crate's `src` tree (including code compiled only in some
configurations/tests); that count is orientation evidence, not an audit finding. Neither the
fetched `README.md` nor `ABOUT.md` claims that the crate is unsafe-free.

## Final verdict

**Can Boa host this API shape for untrusted scripts: yes, with material containment caveats.**
The promise/resolver bridge is straightforward, the Rust-controlled job loop supports real
parallel child-process launch batches, the required modern JavaScript works, and the zero-I/O
surface is achievable by exposing only selected native functions. The tested determinism prelude
held against constructor, reflection, dynamic-code, descriptor, and same-realm attacks, with a
fixed host clock as backup. Runtime limits and `catch_unwind` keep the process alive in the tested
failure modes.

Before treating this as a security boundary, agentstack should (1) make all poison descriptors
non-configurable before any untrusted source runs, (2) consider denying dynamic string compilation
through `HostHooks::ensure_can_compile_strings`, (3) use `evaluate_async_with_budget` plus an
external deadline and discard a timed-out Context, (4) impose host-side prompt/agent/budget limits,
and (5) add an outer isolation boundary if engine memory-safety bugs are in scope. Boa has no
production synchronous fuel/deadline interrupt and no demonstrated general JS heap cap; loop
limits do not preempt a costly single builtin/native operation. Its substantial dependency graph
and internal unsafe code also need explicit approval.

Those last points are the strongest arguments for the QuickJS-in-wasmtime fallback: Wasmtime fuel
or epoch interruption, linear-memory limits, and a WebAssembly isolation boundary provide a
cleaner answer for hard wall-clock/resource containment and reduce the blast radius of engine
memory bugs. Boa remains the simpler integration for this API shape, but the fallback becomes the
better security choice if synchronous hard deadlines, strict memory ceilings, or defense against
native-engine memory unsafety are non-negotiable.
