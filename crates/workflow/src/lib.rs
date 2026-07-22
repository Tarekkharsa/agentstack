//! Governed workflow-orchestration engine (Boa JS) as a self-contained domain.
//!
//! Hostile script text in, brokered spawn-requests out, step results in, final
//! value out. Like `executor`, this crate has **no** internal dependency edges:
//! Boa can never reach `trust`, `policy`, `core`, `adapters`, `recorder`, or any
//! enforcement path. The CLI composes it with the locked-run spawner and the
//! recorder. It owns no thread and no clock it can be denied — the CLI drives it
//! as a state machine, one [`WorkflowRun::step`] at a time.
//!
//! # Honest posture (kept verbatim; see [`POSTURE_LABEL`])
//!
//! Precisely: this is a **compile-time reach** boundary (Boa's code cannot
//! *call* those APIs), not a **runtime memory** boundary. The `workflow` crate
//! links into the `agentstack` process, whose address space also holds the
//! `CommitmentKey` and secrets resolved in-flight by the gateway, so a Boa
//! memory-safety bug is a whole-process concern, not a contained one — the
//! compile edge stops authority reach, only the WASM fallback (§12.2) would add
//! runtime isolation. This is the honest reading of "confined."
//!
//! One residual the "human-reviewed script" framing must not hide, because it
//! is the surface v1 actually keeps: Boa's **parser** only ever sees the trusted
//! pinned script, but Boa's **runtime** processes untrusted string *data* —
//! `agent()` results are model output and `args` come from the invoker, and a
//! trusted script may run string/regex builtins over them (`regress`, the
//! backtracking regex engine, on attacker-influenced input). That is far
//! narrower than `tools_execute` (which evaluates hostile *code*), and disabling
//! dynamic compilation (`ensure_can_compile_strings`) means hostile data can
//! never *become* code — but a runtime/regex bug on hostile string data is
//! reachable, and it is exactly the class the WASM fallback would contain. State
//! it in the posture label; it does not block v1.
//!
//! The hard backstop must therefore be **out-of-thread**: a watchdog thread (or
//! `SIGALRM`) that force-exits the process on wall-clock overrun regardless of
//! what the drive thread is doing; "the CLI records `WorkflowFailed` and exits"
//! is only true if a thread that is *not* stuck in Boa does the recording and
//! the exit. So even a stalled builtin slice cannot outlive the run — via the
//! watchdog, not the cooperative check. No JS heap cap exists in-process; v1
//! states that in the posture label rather than pretending otherwise.

#![forbid(unsafe_code)]

mod bridge;
mod error;
mod meta;

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use boa_engine::builtins::promise::PromiseState;
use boa_engine::context::time::FixedClock;
use boa_engine::context::{ContextBuilder, HostHooks};
use boa_engine::realm::Realm;
use boa_engine::{Context, JsError, JsNativeError, JsNativeErrorKind, JsString, JsValue, Source};

use bridge::{activate, install_agent, js_to_value, value_to_js, PendingSpawn, SpawnState};

pub use error::{WorkflowError, WorkflowErrorKind};
pub use meta::Meta;

/// The verbatim §12.2/§12.3 honesty text, copied byte-for-byte from
/// `docs/design/workflows-capability.md`. It is the single source of the
/// posture claim, and it is duplicated in the crate's module doc-comment so a
/// reviewer sees it at the top of the crate.
///
/// Carry-forward: Stage E (`crates/recorder` + `agentstack workflow report`)
/// renders this verbatim as the per-run posture label; do not fork the wording.
/// When that report lands, the §12.2 runtime-data ReDoS residual (agent()
/// results and invoker args flowing through string/regex builtins) belongs in
/// the same honesty text.
///
/// **Fallback trigger, recorded:** QuickJS-in-wasmtime becomes the right choice
/// if any of these become non-negotiable — hard synchronous deadlines (wasmtime
/// epochs/fuel), strict memory ceilings (linear-memory cap), or containment of
/// engine memory-unsafety (WASM boundary) — i.e. if workflow scripts ever run at
/// less than full trust-gated review.
pub const POSTURE_LABEL: &str = concat!(
    "Precisely: this is a **compile-time reach** boundary (Boa's code cannot ",
    "*call* those APIs), not a **runtime memory** boundary. The `workflow` crate ",
    "links into the `agentstack` process, whose address space also holds the ",
    "`CommitmentKey` and secrets resolved in-flight by the gateway, so a Boa ",
    "memory-safety bug is a whole-process concern, not a contained one — the ",
    "compile edge stops authority reach, only the WASM fallback (§12.2) would add ",
    "runtime isolation. This is the honest reading of \"confined.\"\n\n",
    "One residual the \"human-reviewed script\" framing must not hide, because it ",
    "is the surface v1 actually keeps: Boa's **parser** only ever sees the trusted ",
    "pinned script, but Boa's **runtime** processes untrusted string *data* — ",
    "`agent()` results are model output and `args` come from the invoker, and a ",
    "trusted script may run string/regex builtins over them (`regress`, the ",
    "backtracking regex engine, on attacker-influenced input). That is far ",
    "narrower than `tools_execute` (which evaluates hostile *code*), and disabling ",
    "dynamic compilation (`ensure_can_compile_strings`) means hostile data can ",
    "never *become* code — but a runtime/regex bug on hostile string data is ",
    "reachable, and it is exactly the class the WASM fallback would contain. State ",
    "it in the posture label; it does not block v1.\n\n",
    "The hard backstop must therefore be **out-of-thread**: a watchdog thread (or ",
    "`SIGALRM`) that force-exits the process on wall-clock overrun regardless of ",
    "what the drive thread is doing; \"the CLI records `WorkflowFailed` and exits\" ",
    "is only true if a thread that is *not* stuck in Boa does the recording and ",
    "the exit. So even a stalled builtin slice cannot outlive the run — via the ",
    "watchdog, not the cooperative check. No JS heap cap exists in-process; v1 ",
    "states that in the posture label rather than pretending otherwise.",
);

/// Interpreter ceilings. All host-side, all set before untrusted code runs. The
/// CLI computes these under `MachineLimits` discipline (Stage D); Stage B only
/// consumes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    /// Boa's default is `u64::MAX`, so this MUST be set for containment — an
    /// unset loop limit is the `while(true)` containment bug.
    pub loop_iteration_limit: u64,
    pub recursion_limit: usize,
    pub stack_size_limit: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        // Conservative, finite, and non-`MAX`. Large enough for real
        // orchestration, small enough to contain a runaway loop quickly.
        Self {
            loop_iteration_limit: 10_000_000,
            recursion_limit: 400,
            stack_size_limit: 16 * 1024,
        }
    }
}

/// One brokered child-run request handed to the CLI to execute as a locked run.
/// `id` correlates the eventual [`StepResult`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnRequest {
    pub id: u64,
    pub role: String,
    pub prompt: String,
    pub opts: serde_json::Value,
}

/// A batch of requests the CLI may fan out concurrently (bounded by the
/// engine-owned cap in Stage D).
#[derive(Debug, Clone, PartialEq)]
pub struct StepBatch {
    pub requests: Vec<SpawnRequest>,
}

/// The CLI's answer for one prior [`SpawnRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub request_id: u64,
    pub output: StepOutput,
}

/// Completed resolves the promise with the value; Failed resolves with JS
/// `null` (R1: the script decides severity — the promise is never rejected, so
/// a failed child can never throw an uncatchable rejection into the workflow).
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutput {
    Completed(serde_json::Value),
    Failed,
}

/// The state-machine result of one drive step.
#[derive(Debug)]
pub enum StepOutcome {
    /// Spawns are pending — fan them out, feed the results, call `step` again.
    Batch(StepBatch),
    /// The root promise fulfilled; this is the final value.
    Done(serde_json::Value),
    /// Limit hit, panic, uncaught throw, or engine/host error.
    Failed(WorkflowError),
}

/// A single governed workflow evaluation, driven as a state machine.
pub struct WorkflowRun {
    context: Context,
    state: Rc<RefCell<SpawnState>>,
    meta: Meta,
    /// The script wrapped as an async-IIFE (A1: async function body).
    wrapped_source: String,
    /// The root promise/value, captured on the first `step`.
    root: Option<JsValue>,
    /// Whether the untrusted script has been evaluated yet.
    started: bool,
    /// Whether a terminal `Done`/`Failed` was already returned.
    done: bool,
    /// Whether a panic crossed the `Context`; if so it is discarded, not reused.
    poisoned: bool,
}

impl WorkflowRun {
    /// Parse-only meta extraction, `Context` construction, host-hook + limits
    /// wiring, and prelude install. Does **not** evaluate the untrusted script —
    /// that is deferred to the first [`step`](Self::step), so a synchronous
    /// `while(true)` fails in `step` (as `IterationLimit`), not here.
    ///
    /// Returns `Err` for a parse failure or a meta-rule violation; at that point
    /// nothing untrusted has executed.
    pub fn new(script: &str, limits: RuntimeLimits) -> Result<Self, WorkflowError> {
        // AL5: this is a self-contained domain crate that ingests hostile script
        // text, so it must fail closed at EVERY entry — never rely on its caller
        // to catch a panic. Boa's parser is not a trusted-not-to-panic
        // dependency, so the parse/extraction path is routed through
        // `contain_panic`: a panic becomes `Err(InvalidScript)`, and a panic in
        // the rest of construction becomes `Err(Internal)` — neither unwinds out
        // of `new`. (The parse path has no known reliable panic input, so it is
        // covered by code inspection plus the `contain_panic` witness test.)
        let meta = contain_panic(
            WorkflowError::invalid_script("the workflow parser panicked on hostile input"),
            // Parse-only: no Context exists yet, so no untrusted code can run.
            || meta::extract_meta(script),
        )?;

        contain_panic(
            WorkflowError::internal("workflow interpreter construction panicked"),
            || Self::build(script, limits, meta),
        )
    }

    /// Build the run from already-extracted `meta`. Split out of `new` so the
    /// panic containment (AL5) can wrap it as one unit.
    fn build(script: &str, limits: RuntimeLimits, meta: Meta) -> Result<Self, WorkflowError> {
        let context = build_context(limits)?;
        let state = Rc::new(RefCell::new(SpawnState::new(meta.roles.clone())));

        let mut run = Self {
            context,
            state,
            meta,
            wrapped_source: wrap_for_eval(script),
            root: None,
            started: false,
            done: false,
            poisoned: false,
        };
        run.install()?;
        Ok(run)
    }

    /// The validated metadata (roles/ceilings the CLI needs before spawning).
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// Resolve each prior request's promise, drain the job queue to the next
    /// fixpoint, and report the next batch / completion / failure. The first
    /// call passes an empty `Vec`.
    pub fn step(&mut self, results: Vec<StepResult>) -> StepOutcome {
        if self.poisoned {
            return StepOutcome::Failed(WorkflowError::panicked());
        }
        if self.done {
            return StepOutcome::Failed(WorkflowError::internal("workflow run already finished"));
        }

        // Panic containment: `Context` is not `UnwindSafe`, so the whole drive
        // is wrapped in `AssertUnwindSafe`. Any panic that unwinds through the
        // Context poisons this run — the Context is discarded, never reused, and
        // the process survives (a fresh `WorkflowRun::new` still works).
        let driven = catch_unwind(AssertUnwindSafe(|| self.drive(results)));
        match driven {
            Ok(outcome) => {
                if matches!(outcome, StepOutcome::Done(_) | StepOutcome::Failed(_)) {
                    self.done = true;
                }
                outcome
            }
            Err(_) => {
                self.poisoned = true;
                StepOutcome::Failed(WorkflowError::panicked())
            }
        }
    }

    fn install(&mut self) -> Result<(), WorkflowError> {
        install_agent(&mut self.context)
            .map_err(|_| WorkflowError::internal("failed to install the agent bridge"))?;
        // The prelude is a trusted, pre-parsed host Source; the compile-strings
        // denial does not block it. Poisoning runs before any untrusted code.
        self.context
            .eval(Source::from_bytes(PRELUDE))
            .map_err(|_| WorkflowError::internal("failed to install the workflow prelude"))?;
        Ok(())
    }

    fn drive(&mut self, results: Vec<StepResult>) -> StepOutcome {
        // Make this run's state visible to the capture-free `agent()` native for
        // the duration of this synchronous drive; the guard pops it on return.
        let _active = activate(&self.state);

        // First step: evaluate the untrusted script to its first suspension.
        if !self.started {
            self.started = true;
            match self
                .context
                .eval(Source::from_bytes(self.wrapped_source.as_str()))
            {
                Ok(value) => self.root = Some(value),
                Err(err) => return StepOutcome::Failed(self.classify_engine_error(&err)),
            }
        }

        if let Err(err) = self.resolve_results(results) {
            return StepOutcome::Failed(err);
        }

        if let Err(err) = self.context.run_jobs() {
            return StepOutcome::Failed(self.classify_engine_error(&err));
        }

        // A role the script did not declare was named at the bridge (R2).
        if let Some(role) = self.state.borrow().undeclared.clone() {
            return StepOutcome::Failed(WorkflowError::undeclared_role(&role));
        }

        let pending = self.state.borrow_mut().take_pending();
        if !pending.is_empty() {
            return StepOutcome::Batch(self.build_batch(pending));
        }

        self.settle_root()
    }

    fn resolve_results(&mut self, results: Vec<StepResult>) -> Result<(), WorkflowError> {
        for result in results {
            // A3: an unknown / duplicate / stale id is a clean Internal error —
            // never a panic and never a silent double-resolve.
            let resolvers = self
                .state
                .borrow_mut()
                .awaiting
                .remove(&result.request_id)
                .ok_or_else(|| {
                    WorkflowError::internal("step result for an unknown or already-resolved id")
                })?;

            let value = match result.output {
                // Depth-bounded (A2); refuses adversarial nesting.
                StepOutput::Completed(v) => value_to_js(&v, &mut self.context)?,
                // R1: a failed child resolves with `null`, never a rejection.
                StepOutput::Failed => JsValue::null(),
            };

            resolvers
                .resolve
                .call(&JsValue::undefined(), &[value], &mut self.context)
                .map_err(|_| WorkflowError::internal("failed to resolve a step promise"))?;
        }
        Ok(())
    }

    fn build_batch(&self, pending: Vec<PendingSpawn>) -> StepBatch {
        let requests = pending
            .into_iter()
            .map(|p| SpawnRequest {
                id: p.id,
                role: p.role,
                prompt: p.prompt,
                opts: p.opts,
            })
            .collect();
        StepBatch { requests }
    }

    fn settle_root(&mut self) -> StepOutcome {
        let root = match self.root.clone() {
            Some(root) => root,
            None => return StepOutcome::Failed(WorkflowError::internal("root value missing")),
        };

        let Some(promise) = root.as_promise() else {
            // The script escaped the async wrapper; use the completion value.
            return match js_to_value(&root, &mut self.context, 0) {
                Ok(value) => StepOutcome::Done(value),
                Err(err) => StepOutcome::Failed(err),
            };
        };

        match promise.state() {
            PromiseState::Pending => StepOutcome::Failed(WorkflowError::internal(
                "workflow stalled: root promise pending with no pending spawns",
            )),
            PromiseState::Fulfilled(value) => match js_to_value(&value, &mut self.context, 0) {
                Ok(value) => StepOutcome::Done(value),
                Err(err) => StepOutcome::Failed(err),
            },
            PromiseState::Rejected(reason) => StepOutcome::Failed(self.classify_rejection(&reason)),
        }
    }

    /// Classify an error surfaced directly from `eval` / `run_jobs`. A Boa
    /// `RuntimeLimit` (loop / recursion / stack) is non-catchable and arrives
    /// here rather than as a promise rejection.
    fn classify_engine_error(&mut self, err: &JsError) -> WorkflowError {
        if let Ok(native) = err.try_native(&mut self.context) {
            if matches!(native.kind, JsNativeErrorKind::RuntimeLimit) {
                return WorkflowError::iteration_limit("interpreter execution limit exceeded");
            }
        }
        WorkflowError::runtime_error(err.to_string())
    }

    /// Classify a root-promise rejection reason. The compile-strings hook tags
    /// its refusal with a sentinel so a denied `eval`/`Function(string)` is
    /// reported as `CompileDenied`.
    ///
    /// AL6: this runs ONLY on an already-rejected root, so a script that forges
    /// the sentinel string (e.g. `throw new Error("agentstack:compile-denied")`)
    /// can at worst mislabel the failure's KIND as `CompileDenied` — it can never
    /// turn a `Failed` into `Done`, because reaching here already means the run
    /// failed. Stage C hardening: replace this substring match with a structured,
    /// non-forgeable tag (a distinct native-error marker set only by the host
    /// hook) so a script cannot even mislabel the kind.
    fn classify_rejection(&mut self, reason: &JsValue) -> WorkflowError {
        let text = reason
            .to_string(&mut self.context)
            .map(|s| s.to_std_string_lossy())
            .unwrap_or_default();
        if text.contains(COMPILE_DENIED_SENTINEL) {
            return WorkflowError::compile_denied();
        }
        WorkflowError::runtime_error(text)
    }
}

const PRELUDE: &str = include_str!("prelude.js");

/// Run `f`, converting any panic that unwinds out of it into `Err(on_panic)`;
/// the closure's own `Err` passes through unchanged. This is the AL5
/// containment seam `WorkflowRun::new` uses so hostile script text can never
/// panic *out* of construction. `f` touches no live `Context` (the parse path
/// has none; construction owns a fresh one that is dropped on panic), so
/// `AssertUnwindSafe` is sound here.
fn contain_panic<T>(
    on_panic: WorkflowError,
    f: impl FnOnce() -> Result<T, WorkflowError>,
) -> Result<T, WorkflowError> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(_) => Err(on_panic),
    }
}

/// Sentinel embedded in the compile-strings refusal so a rejection carrying it
/// can be classified back to [`WorkflowErrorKind::CompileDenied`].
const COMPILE_DENIED_SENTINEL: &str = "agentstack:compile-denied";

/// Wrap the script as an async-IIFE so top-level `await` / `return` are legal
/// and the completion value is the root promise (A1: async function body).
///
/// AL4: the leading `export ` of the §3 export-meta form is stripped first —
/// `export` is illegal inside the IIFE body. `meta::deexport` removes only that
/// one keyword; no other byte of the script changes.
fn wrap_for_eval(script: &str) -> String {
    format!("(async () => {{\n{}\n}})()", meta::deexport(script))
}

fn build_context(limits: RuntimeLimits) -> Result<Context, WorkflowError> {
    let mut context = ContextBuilder::new()
        // Denies `eval(string)` / `new Function(string)` from Context creation.
        .host_hooks(Rc::new(Hooks))
        // Host-level deterministic-time backstop behind the JS poisoning.
        .clock(Rc::new(FixedClock::from_millis(0)))
        .build()
        .map_err(|_| WorkflowError::internal("failed to build the interpreter context"))?;

    // Boa's loop-iteration default is u64::MAX; setting it is the containment.
    let runtime_limits = context.runtime_limits_mut();
    runtime_limits.set_loop_iteration_limit(limits.loop_iteration_limit);
    runtime_limits.set_recursion_limit(limits.recursion_limit);
    runtime_limits.set_stack_size_limit(limits.stack_size_limit);

    Ok(context)
}

/// Host hooks: the whole point of this type is to deny dynamic string
/// compilation. Every other hook keeps its default.
#[derive(Debug)]
struct Hooks;

impl HostHooks for Hooks {
    fn ensure_can_compile_strings(
        &self,
        _realm: Realm,
        _parameters: &[JsString],
        _body: &JsString,
        _direct: bool,
        _context: &mut Context,
    ) -> boa_engine::JsResult<()> {
        Err(JsError::from(JsNativeError::typ().with_message(format!(
            "{COMPILE_DENIED_SENTINEL}: dynamic string compilation is disabled in workflows"
        ))))
    }
}

#[cfg(test)]
impl WorkflowRun {
    /// Test-only seam: install a global native that panics, to prove native
    /// panics are contained (witness 6). Not part of the public surface.
    fn install_panic_probe(&mut self) {
        use boa_engine::{js_string, NativeFunction};
        let probe = NativeFunction::from_fn_ptr(|_this, _args, _context| {
            panic!("intentional native panic (test probe)")
        });
        self.context
            .register_global_builtin_callable(js_string!("__panic_probe"), 0, probe)
            .expect("register panic probe");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_to_done(script: &str) -> serde_json::Value {
        let mut run = WorkflowRun::new(script, RuntimeLimits::default()).expect("script parses");
        match run.step(Vec::new()) {
            StepOutcome::Done(value) => value,
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn no_ambient_authority() {
        // Witness 2: no fs / net / env / process intrinsic exists in the context.
        let script = "const meta = { roles: [] };\n\
             return [typeof require, typeof process, typeof fetch, typeof Deno, \
             typeof globalThis.Bun];";
        let value = run_to_done(script);
        let entries = value.as_array().expect("array result");
        assert_eq!(entries.len(), 5);
        for entry in entries {
            assert_eq!(entry, "undefined");
        }
    }

    #[test]
    fn infinite_loop_hits_iteration_limit() {
        // Witness 4: while(true) hits the non-catchable ceiling; the catch is
        // bypassed; the engine survives (a fresh run reaches Done).
        let script = "const meta = { roles: [] };\n\
             try { while (true) {} } catch (e) { globalThis.__leaked = true; }\n\
             return 1;";
        let mut run = WorkflowRun::new(script, RuntimeLimits::default()).unwrap();
        match run.step(Vec::new()) {
            StepOutcome::Failed(err) => {
                assert_eq!(err.kind, WorkflowErrorKind::IterationLimit)
            }
            other => panic!("expected Failed(IterationLimit), got {other:?}"),
        }
        // The engine survived: an independent run completes normally.
        assert_eq!(run_to_done("const meta = { roles: [] };\nreturn 42;"), 42);
    }

    #[test]
    fn native_panic_fails_closed() {
        // Witness 6: a panicking native fails the run closed; no unwind escapes
        // the crate; the poisoned run refuses reuse; the process continues.
        let mut run = WorkflowRun::new(
            "const meta = { roles: [] };\n__panic_probe();\nreturn 1;",
            RuntimeLimits::default(),
        )
        .unwrap();
        run.install_panic_probe();

        // Silence the default panic hook's noise for the expected panic.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = run.step(Vec::new());
        std::panic::set_hook(previous);

        match outcome {
            StepOutcome::Failed(err) => assert_eq!(err.kind, WorkflowErrorKind::Panicked),
            other => panic!("expected Failed(Panicked), got {other:?}"),
        }
        // The poisoned run refuses reuse.
        assert!(matches!(
            run.step(Vec::new()),
            StepOutcome::Failed(err) if err.kind == WorkflowErrorKind::Panicked
        ));
        // The process is fine: an independent run still works.
        assert_eq!(run_to_done("const meta = { roles: [] };\nreturn 7;"), 7);
    }

    #[test]
    fn determinism_apis_denied_and_unrestorable() {
        // Witness 7: Date.now / argless new Date / Math.random are denied, and
        // no restoration path succeeds from inside the script.
        let script = "const meta = { roles: [] };\n\
             const out = [];\n\
             const probe = (fn) => { try { fn(); out.push('LEAK'); } catch (e) { out.push('denied'); } };\n\
             probe(() => Date.now());\n\
             probe(() => new Date());\n\
             probe(() => Math.random());\n\
             probe(() => ({}).constructor.constructor('return Date.now')());\n\
             probe(() => Reflect.get(Date, 'now')());\n\
             probe(() => { Object.defineProperty(Math, 'random', { value: () => 0.5 }); return Math.random(); });\n\
             probe(() => { delete globalThis.Date; return Date.now(); });\n\
             return out;";
        let value = run_to_done(script);
        let entries = value.as_array().expect("array result");
        assert!(!entries.is_empty());
        for entry in entries {
            assert_eq!(entry, "denied", "an access leaked: {value:?}");
        }
    }

    #[test]
    fn dynamic_compilation_is_denied() {
        // The compile-strings hook turns a script `eval` into CompileDenied.
        let mut run = WorkflowRun::new(
            "const meta = { roles: [] };\nreturn eval('1 + 1');",
            RuntimeLimits::default(),
        )
        .unwrap();
        match run.step(Vec::new()) {
            StepOutcome::Failed(err) => assert_eq!(err.kind, WorkflowErrorKind::CompileDenied),
            other => panic!("expected Failed(CompileDenied), got {other:?}"),
        }
    }

    #[test]
    fn parallel_batches_agents_in_one_drain() {
        // Bridge batching + witness 1 bridge path.
        let script = "const meta = { roles: ['r'] };\n\
             const outs = await parallel([\n\
               () => agent('a', { role: 'r' }),\n\
               () => agent('b', { role: 'r' }),\n\
               () => agent('c', { role: 'r' }),\n\
             ]);\n\
             return outs;";
        let mut run = WorkflowRun::new(script, RuntimeLimits::default()).unwrap();

        let batch = match run.step(Vec::new()) {
            StepOutcome::Batch(batch) => batch,
            other => panic!("expected Batch, got {other:?}"),
        };
        assert_eq!(batch.requests.len(), 3);
        assert!(batch.requests.iter().all(|r| r.role == "r"));
        let prompts: Vec<&str> = batch.requests.iter().map(|r| r.prompt.as_str()).collect();
        assert_eq!(prompts, ["a", "b", "c"]);

        // Feed uppercase results back, keyed by request id.
        let results = batch
            .requests
            .iter()
            .map(|r| StepResult {
                request_id: r.id,
                output: StepOutput::Completed(serde_json::Value::String(r.prompt.to_uppercase())),
            })
            .collect();
        match run.step(results) {
            StepOutcome::Done(value) => {
                assert_eq!(value, serde_json::json!(["A", "B", "C"]))
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn undeclared_role_is_refused_at_the_bridge() {
        // Witness 1 (bridge side): a role the script did not declare is refused.
        let script = "const meta = { roles: ['r'] };\n\
             const outs = await parallel([() => agent('x', { role: 'other' })]);\n\
             return outs;";
        let mut run = WorkflowRun::new(script, RuntimeLimits::default()).unwrap();
        match run.step(Vec::new()) {
            StepOutcome::Failed(err) => assert_eq!(err.kind, WorkflowErrorKind::UndeclaredRole),
            other => panic!("expected Failed(UndeclaredRole), got {other:?}"),
        }
    }

    #[test]
    fn deeply_nested_step_result_is_refused() {
        // A2: a pathologically nested step result is refused cleanly, no crash.
        let script = "const meta = { roles: ['r'] };\n\
             const x = await agent('p', { role: 'r' });\n\
             return x;";
        let mut run = WorkflowRun::new(script, RuntimeLimits::default()).unwrap();
        let batch = match run.step(Vec::new()) {
            StepOutcome::Batch(batch) => batch,
            other => panic!("expected Batch, got {other:?}"),
        };
        let id = batch.requests[0].id;

        // Nest well past MAX_JSON_DEPTH (128); dropping ~300 frames is safe.
        let mut adversarial = serde_json::Value::Null;
        for _ in 0..300 {
            adversarial = serde_json::Value::Array(vec![adversarial]);
        }
        let outcome = run.step(vec![StepResult {
            request_id: id,
            output: StepOutput::Completed(adversarial),
        }]);
        match outcome {
            StepOutcome::Failed(err) => assert_eq!(err.kind, WorkflowErrorKind::Internal),
            other => panic!("expected Failed(Internal), got {other:?}"),
        }
    }

    #[test]
    fn pipeline_runs_each_item_through_all_stages() {
        // AL2: pipeline is per-item, not a waterfall. Each item flows through
        // all stages independently; a stage callback sees (prevResult,
        // originalItem, index); a stage that throws drops THAT item to null and
        // skips its remaining stages, leaving the others intact.
        let script = "const meta = { roles: [] };\n\
             const out = await pipeline([1, 2, 3],\n\
               (v, item, i) => { if (item === 2) throw new Error('drop'); return v * 10 + i; },\n\
               (v) => v + 100);\n\
             return out;";
        // item 1 (index 0): 1*10+0=10 -> 110 ; item 2 throws -> null ;
        // item 3 (index 2): 3*10+2=32 -> 132.
        assert_eq!(run_to_done(script), serde_json::json!([110, null, 132]));
    }

    #[test]
    fn parallel_resolves_a_throwing_thunk_to_null() {
        // AL3: a throwing thunk resolves that slot to null; parallel never
        // rejects, so the surviving thunks still resolve normally.
        let script = "const meta = { roles: [] };\n\
             const out = await parallel([\n\
               () => 1,\n\
               () => { throw new Error('boom'); },\n\
               async () => 3,\n\
             ]);\n\
             return out;";
        assert_eq!(run_to_done(script), serde_json::json!([1, null, 3]));
    }

    #[test]
    fn export_meta_form_parses_and_runs() {
        // AL4: `export const meta = {…}` (the §3 shape) yields the same Meta as
        // the bare form and still evaluates to Done.
        let bare = "const meta = { roles: ['r'] };\nreturn 7;";
        let exported = "export const meta = { roles: ['r'] };\nreturn 7;";

        let bare_meta = WorkflowRun::new(bare, RuntimeLimits::default())
            .expect("bare parses")
            .meta()
            .clone();
        let exported_meta = WorkflowRun::new(exported, RuntimeLimits::default())
            .expect("export form parses")
            .meta()
            .clone();
        assert_eq!(bare_meta, exported_meta);

        assert_eq!(run_to_done(exported), 7);
    }

    #[test]
    fn parse_path_panic_is_contained() {
        // AL5 witness: a panic routed through the SAME containment helper that
        // `WorkflowRun::new` uses becomes an `Err`, never an unwind. Boa's parser
        // has no known reliable panic input, so the real parse path is covered by
        // code inspection plus this seam exercising `contain_panic` directly.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let out: Result<(), WorkflowError> =
            contain_panic(WorkflowError::invalid_script("probe"), || {
                panic!("intentional panic through the containment seam")
            });
        std::panic::set_hook(previous);
        assert_eq!(out.unwrap_err().kind, WorkflowErrorKind::InvalidScript);
    }

    #[test]
    fn forged_sentinel_cannot_flip_failed_to_done() {
        // AL6: a script that throws an error carrying the compile-denied sentinel
        // ends in Failed (kind CompileDenied — the accepted mislabel), NEVER Done.
        // classify_rejection runs only on an already-rejected root, so the forgery
        // can mislabel the kind but cannot manufacture success.
        let script = format!(
            "const meta = {{ roles: [] }};\nthrow new Error(\"{COMPILE_DENIED_SENTINEL} forged\");\n"
        );
        let mut run = WorkflowRun::new(&script, RuntimeLimits::default()).unwrap();
        match run.step(Vec::new()) {
            StepOutcome::Failed(err) => assert_eq!(err.kind, WorkflowErrorKind::CompileDenied),
            other => panic!("expected Failed(CompileDenied), got {other:?}"),
        }
    }
}
